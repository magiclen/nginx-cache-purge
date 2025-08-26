use std::{
    fs::Permissions,
    io,
    io::IsTerminal,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context as AnyhowContext};
use axum::{
    extract::OriginalUri, // 添加这个导入
    http::{header, HeaderValue, StatusCode }, // 确保有 Uri
    response::IntoResponse,
    routing::any,
    Router,
};
use axum_extra::extract::Query;
use serde::Deserialize;
use tokio::{fs, net::UnixListener};
use tower_http::{
    set_header::SetResponseHeaderLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::Level;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::{purge, uds_serve::serve, AppResult};

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
    OriginalUri(original_uri): OriginalUri,
    Query(Args {
        cache_path,
        levels,
        mut key,
        remove_first,
        exclude_keys,
    }): Query<Args>,
) -> impl IntoResponse {
    // 调试：打印原始 URI
    println!("Original URI: {}", original_uri);

    // 从原始 URI 中手动提取完整的 key 参数（包含所有重复参数）
    if let Some(query) = original_uri.query() {
        println!("Raw query string: {}", query);

        // 手动查找第一个 key 参数的位置
        if let Some(key_start) = find_first_key_param(query) {
            // 提取 key 参数的值
            let key_value = extract_key_value(&query[key_start..]);
            if let Some(extracted_key) = key_value {
                key = extracted_key;
                println!("Manually extracted full key: {}", key);
            }
        }
    }

    println!("Final key to process: {}", key);

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




// 查找第一个 key 参数的起始位置
fn find_first_key_param(query: &str) -> Option<usize> {
    let mut pos = 0;
    loop {
        // 查找 "key=" 的位置
        if let Some(index) = query[pos..].find("key=") {
            let actual_pos = pos + index;
            // 确保这是参数的开始（要么在字符串开始，要么前面是 &）
            if actual_pos == 0 || query.as_bytes()[actual_pos - 1] == b'&' {
                return Some(actual_pos + 4); // 跳过 "key=" 四个字符
            }
            pos = actual_pos + 1;
        } else {
            break;
        }
    }
    None
}

// 从 key= 之后提取完整的值（处理 URL 编码和多个 key 参数的情况）
fn extract_key_value(query_part: &str) -> Option<String> {
    // 查找下一个 &key= 或者字符串结尾，作为参数的结束
    let end_pos = query_part[1..] // 跳过第一个字符避免匹配到自己
        .find("&key=")
        .map(|pos| pos + 1) // 调整位置
        .unwrap_or(query_part.len());

    let encoded_value = &query_part[..end_pos];

    // URL 解码
    match urlencoding::decode(encoded_value) {
        Ok(decoded) => Some(decoded.into_owned()),
        Err(_) => Some(encoded_value.to_string()), // 解码失败则返回原始值
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
    serve(uds, app).await?;

    // let addr = "127.0.0.1:3000";
    // let listener = tokio::net::TcpListener::bind(addr).await?;
    // tracing::info!("listening on http://{addr}");
    // axum::serve(listener, app).await?;

    Ok(AppResult::Ok)
}
