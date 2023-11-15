use std::{
    fs::Permissions,
    io,
    io::IsTerminal,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    pin::Pin,
    task::{ready, Context, Poll},
};

use anyhow::{anyhow, Context as AnyhowContext};
use axum::{
    http::{header, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::any,
    BoxError, Router,
};
use axum_extra::extract::Query;
use hyper::server::accept::Accept;
use serde::Deserialize;
use tokio::{
    fs,
    net::{UnixListener, UnixStream},
};
use tower_http::{
    set_header::SetResponseHeaderLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::Level;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::{purge, AppResult};

struct ServerAccept {
    uds: UnixListener,
}

impl Accept for ServerAccept {
    type Conn = UnixStream;
    type Error = BoxError;

    fn poll_accept(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Self::Conn, Self::Error>>> {
        let (stream, _addr) = ready!(self.uds.poll_accept(cx))?;

        Poll::Ready(Some(Ok(stream)))
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OneOrManyString {
    One(String),
    Many(Vec<String>),
}

impl From<OneOrManyString> for Vec<String> {
    #[inline]
    fn from(value: OneOrManyString) -> Self {
        match value {
            OneOrManyString::One(s) => vec![s],
            OneOrManyString::Many(v) => v,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Args {
    cache_path:   PathBuf,
    levels:       String,
    key:          String,
    remove_first: Option<String>,
    exclude_keys: Option<OneOrManyString>,
}

async fn index_handler(
    Query(Args {
        cache_path,
        levels,
        mut key,
        remove_first,
        exclude_keys,
    }): Query<Args>,
) -> impl IntoResponse {
    if let Some(remove_first) = remove_first {
        if let Some(index) = key.find(remove_first.as_str()) {
            key.replace_range(index..index + remove_first.len(), "");
        }
    }

    match purge(cache_path, levels, key, exclude_keys.map(|e| e.into()).unwrap_or_else(Vec::new))
        .await
    {
        Ok(result) => match result {
            AppResult::Ok => (StatusCode::OK, "Ok.".to_string()),
            _ => (StatusCode::ACCEPTED, "No cache needs to be purged.".to_string()),
        },
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{error:?}")),
    }
}

fn create_app() -> Router {
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
}

pub async fn server_main(socket_file_path: &Path) -> anyhow::Result<AppResult> {
    let mut ansi_color = io::stdout().is_terminal();

    if ansi_color && enable_ansi_support::enable_ansi_support().is_err() {
        ansi_color = false;
    }

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(ansi_color))
        .with(EnvFilter::builder().with_default_directive(Level::INFO.into()).from_env_lossy())
        .init();

    let app = create_app();

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

    axum::Server::builder(ServerAccept {
        uds,
    })
    .serve(app.into_make_service())
    .await?;

    // use std::str::FromStr;
    // axum::Server::bind(&std::net::SocketAddr::from_str("127.0.0.1:3000").unwrap())
    //     .serve(app.into_make_service())
    //     .await?;

    Ok(AppResult::Ok)
}
