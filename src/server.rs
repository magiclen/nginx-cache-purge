use std::{
    collections::HashMap,
    fs::Permissions,
    io,
    io::IsTerminal,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    str,
    sync::Arc,
};

use anyhow::{Context as AnyhowContext, anyhow};
use axum::{
    Router,
    extract::{RawQuery, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::IntoResponse,
    routing::any,
};
use tokio::{fs, net::UnixListener};
use tower_http::{
    set_header::SetResponseHeaderLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::Level;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{AppResult, purge, uds_serve::serve};

const HEADER_ZONE: &str = "x-cache-zone";
const HEADER_CACHE_PATH: &str = "x-cache-path";
const HEADER_LEVELS: &str = "x-cache-levels";
const HEADER_KEY: &str = "x-cache-key";
const HEADER_REMOVE_FIRST: &str = "x-remove-first";
const HEADER_EXCLUDE_KEY: &str = "x-exclude-key";

#[derive(Debug)]
pub struct Zone {
    cache_path: PathBuf,
    levels:     String,
}

pub type Zones = HashMap<String, Zone>;

/// Turn the flat `NAME PATH LEVELS` triples coming from the CLI into a zone table.
pub fn parse_zones(zones: &[String]) -> anyhow::Result<Zones> {
    let mut result = Zones::with_capacity(zones.len() / 3);

    for zone in zones.chunks(3) {
        let [name, cache_path, levels] = zone else {
            return Err(anyhow!("A cache zone should be defined as `NAME PATH LEVELS`."));
        };

        if result.contains_key(name) {
            return Err(anyhow!("The cache zone {name:?} is defined more than once."));
        }

        result.insert(name.clone(), Zone {
            cache_path: PathBuf::from(cache_path),
            levels:     levels.clone(),
        });
    }

    Ok(result)
}

#[derive(Debug, Default)]
struct Args {
    zone:         Option<String>,
    cache_path:   Option<String>,
    levels:       Option<String>,
    key:          Option<String>,
    remove_first: Option<String>,
    exclude_keys: Vec<String>,
}

/// Parse the query of a purge request.
///
/// Values are taken as they are, without percent-decoding, because nginx builds its cache key from the raw `$request_uri`. The `key` field extends to the end of the query so that a key containing `&` or `?` is not cut short.
fn parse_query(query: &str) -> Args {
    let mut args = Args::default();

    let mut rest = query;

    while !rest.is_empty() {
        let pair_len = rest.find('&').unwrap_or(rest.len());
        let pair = &rest[..pair_len];

        let (name, value_start) = match pair.find('=') {
            Some(index) => (&pair[..index], index + 1),
            None => (pair, pair_len),
        };

        if name == "key" {
            args.key = Some(rest[value_start..].to_string());

            break;
        }

        let value = &rest[value_start..pair_len];

        match name {
            "zone" => args.zone = Some(value.to_string()),
            "cache_path" => args.cache_path = Some(value.to_string()),
            "levels" => args.levels = Some(value.to_string()),
            "remove_first" => args.remove_first = Some(value.to_string()),
            "exclude_keys" | "exclude_key" => args.exclude_keys.push(value.to_string()),
            _ => (),
        }

        rest = rest.get((pair_len + 1)..).unwrap_or("");
    }

    args
}

#[inline]
fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> anyhow::Result<Option<&'a str>> {
    match headers.get(name) {
        Some(value) => str::from_utf8(value.as_bytes())
            .map(Some)
            .with_context(|| anyhow!("The {name} header is not valid UTF-8.")),
        None => Ok(None),
    }
}

/// Let the headers of a purge request override its query, since a header value needs no escaping at all.
fn apply_headers(args: &mut Args, headers: &HeaderMap) -> anyhow::Result<()> {
    for (name, field) in [
        (HEADER_ZONE, &mut args.zone),
        (HEADER_CACHE_PATH, &mut args.cache_path),
        (HEADER_LEVELS, &mut args.levels),
        (HEADER_KEY, &mut args.key),
        (HEADER_REMOVE_FIRST, &mut args.remove_first),
    ] {
        if let Some(value) = header_str(headers, name)? {
            *field = Some(value.to_string());
        }
    }

    let mut exclude_keys = Vec::new();

    for value in headers.get_all(HEADER_EXCLUDE_KEY) {
        exclude_keys.push(
            str::from_utf8(value.as_bytes())
                .with_context(|| anyhow!("The {HEADER_EXCLUDE_KEY} header is not valid UTF-8."))?
                .to_string(),
        );
    }

    if !exclude_keys.is_empty() {
        args.exclude_keys = exclude_keys;
    }

    Ok(())
}

fn resolve_target(zones: &Zones, args: &Args) -> anyhow::Result<(PathBuf, String)> {
    if zones.is_empty() {
        if args.zone.is_some() {
            return Err(anyhow!("This server has no cache zone defined."));
        }

        let cache_path =
            args.cache_path.as_deref().ok_or_else(|| anyhow!("The cache_path is not assigned."))?;

        Ok((PathBuf::from(cache_path), args.levels.clone().unwrap_or_default()))
    } else {
        if args.cache_path.is_some() || args.levels.is_some() {
            return Err(anyhow!(
                "This server only accepts a cache zone name, not a cache path of its own."
            ));
        }

        let name = args.zone.as_deref().ok_or_else(|| anyhow!("The zone is not assigned."))?;

        let zone =
            zones.get(name).ok_or_else(|| anyhow!("The cache zone {name:?} is not defined."))?;

        Ok((zone.cache_path.clone(), zone.levels.clone()))
    }
}

async fn index_handler(
    State(zones): State<Arc<Zones>>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> impl IntoResponse {
    let mut args = parse_query(query.as_deref().unwrap_or(""));

    if let Err(error) = apply_headers(&mut args, &headers) {
        return (StatusCode::BAD_REQUEST, format!("{error:#}"));
    }

    let (cache_path, levels) = match resolve_target(&zones, &args) {
        Ok(target) => target,
        Err(error) => return (StatusCode::BAD_REQUEST, format!("{error:#}")),
    };

    let Some(mut key) = args.key else {
        return (StatusCode::BAD_REQUEST, "The key is not assigned.".to_string());
    };

    if let Some(remove_first) = args.remove_first.as_deref()
        && let Some(stripped) = key.strip_prefix(remove_first)
    {
        key = stripped.to_string();
    }

    let exclude_keys = args.exclude_keys;

    let result = tokio::task::spawn_blocking(move || {
        let exclude_keys: Vec<&str> = exclude_keys.iter().map(|s| s.as_str()).collect();

        purge(cache_path, levels.as_str(), key.as_str(), &exclude_keys)
    })
    .await;

    match result {
        Ok(Ok(AppResult::Ok)) => (StatusCode::OK, "Ok.".to_string()),
        Ok(Ok(_)) => (StatusCode::ACCEPTED, "No cache needs to be purged.".to_string()),
        Ok(Err(error)) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{error:?}")),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{error:?}")),
    }
}

fn create_app(zones: Zones) -> Router {
    Router::new()
        .route("/", any(index_handler))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_request(DefaultOnRequest::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .with_state(Arc::new(zones))
}

pub async fn server_main(socket_file_path: &Path, zones: Zones) -> anyhow::Result<AppResult> {
    let mut ansi_color = io::stdout().is_terminal();

    if ansi_color && enable_ansi_support::enable_ansi_support().is_err() {
        ansi_color = false;
    }

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(ansi_color))
        .with(EnvFilter::builder().with_default_directive(Level::INFO.into()).from_env_lossy())
        .init();

    if zones.is_empty() {
        tracing::warn!(
            "no cache zone is defined, so any client of this socket can purge any directory; \
             consider using --zone"
        );
    }

    let app = create_app(zones);

    let uds = {
        match fs::metadata(socket_file_path).await {
            Ok(metadata) => {
                if metadata.file_type().is_socket() {
                    fs::remove_file(socket_file_path)
                        .await
                        .with_context(|| anyhow!("{socket_file_path:?}"))?;
                } else {
                    return Err(anyhow!("{socket_file_path:?} exists but it is not a socket file"));
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // do nothing
            },
            Err(error) => {
                return Err(error).with_context(|| anyhow!("{socket_file_path:?}"));
            },
        }

        let uds = UnixListener::bind(socket_file_path)
            .with_context(|| anyhow!("{socket_file_path:?}"))?;

        fs::set_permissions(socket_file_path, Permissions::from_mode(0o777))
            .await
            .with_context(|| anyhow!("{socket_file_path:?}"))?;

        uds
    };

    tracing::info!("listening on {socket_file_path:?}");
    serve(uds, app).await?;

    Ok(AppResult::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_query_keeps_the_whole_key() {
        // this is what nginx produces for `PURGE /spudetail/40978870?country=UK&pageNum=1`
        let args = parse_query(
            "cache_path=/api2&levels=1:2&key=example.com/spudetail/40978870?country=UK&pageNum=1",
        );

        assert_eq!(Some("/api2".to_string()), args.cache_path);
        assert_eq!(Some("1:2".to_string()), args.levels);
        assert_eq!(
            Some("example.com/spudetail/40978870?country=UK&pageNum=1".to_string()),
            args.key
        );
    }

    #[test]
    fn parse_query_does_not_decode_values() {
        // nginx builds its cache key from the raw $request_uri, so `+` and `%20` have to survive
        let args = parse_query("levels=1:2&key=https/search?q=a+b%20c");

        assert_eq!(Some("https/search?q=a+b%20c".to_string()), args.key);
    }

    #[test]
    fn parse_query_collects_exclude_keys() {
        let args = parse_query("exclude_keys=http/static/*&exclude_key=http/1&key=http/*");

        assert_eq!(vec!["http/static/*".to_string(), "http/1".to_string()], args.exclude_keys);
        assert_eq!(Some("http/*".to_string()), args.key);
    }

    #[test]
    fn parse_query_handles_a_missing_or_empty_key() {
        assert_eq!(None, parse_query("").key);
        assert_eq!(None, parse_query("levels=1:2").key);
        assert_eq!(Some(String::new()), parse_query("levels=1:2&key=").key);
        assert_eq!(Some(String::new()), parse_query("levels=1:2&key").key);
    }

    #[test]
    fn headers_override_the_query() {
        let mut args = parse_query("cache_path=/wrong&key=wrong");

        let mut headers = HeaderMap::new();
        headers.insert(HEADER_CACHE_PATH, HeaderValue::from_static("/tmp/cache"));
        headers.insert(HEADER_KEY, HeaderValue::from_static("https/a?b=1&c=2"));
        headers.append(HEADER_EXCLUDE_KEY, HeaderValue::from_static("https/static/*"));
        headers.append(HEADER_EXCLUDE_KEY, HeaderValue::from_static("https/1"));

        apply_headers(&mut args, &headers).unwrap();

        assert_eq!(Some("/tmp/cache".to_string()), args.cache_path);
        assert_eq!(Some("https/a?b=1&c=2".to_string()), args.key);
        assert_eq!(vec!["https/static/*".to_string(), "https/1".to_string()], args.exclude_keys);
    }

    #[test]
    fn resolve_target_uses_the_request_when_no_zone_is_defined() {
        let zones = Zones::new();

        let args = Args {
            cache_path: Some("/tmp/cache".to_string()),
            levels: Some("1:2".to_string()),
            ..Args::default()
        };

        assert_eq!(
            (PathBuf::from("/tmp/cache"), "1:2".to_string()),
            resolve_target(&zones, &args).unwrap()
        );

        assert!(resolve_target(&zones, &Args::default()).is_err());
    }

    #[test]
    fn resolve_target_only_accepts_a_zone_name_when_zones_are_defined() {
        let zones =
            parse_zones(&["my_cache".to_string(), "/tmp/cache".to_string(), "1:2".to_string()])
                .unwrap();

        let args = Args {
            zone: Some("my_cache".to_string()),
            ..Args::default()
        };

        assert_eq!(
            (PathBuf::from("/tmp/cache"), "1:2".to_string()),
            resolve_target(&zones, &args).unwrap()
        );

        // a request must not be able to name a cache path of its own
        let args = Args {
            cache_path: Some("/etc".to_string()),
            ..Args::default()
        };

        assert!(resolve_target(&zones, &args).is_err());

        let args = Args {
            zone: Some("other".to_string()),
            ..Args::default()
        };

        assert!(resolve_target(&zones, &args).is_err());
    }
}
