use std::{future::Future, time::Duration};

use axum::{Router, extract::Request};
use hyper::body::Incoming;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::{self, graceful::GracefulShutdown},
};
use tokio::{
    net::UnixListener,
    signal::unix::{SignalKind, signal},
};
use tower::Service;

/// How long to wait before accepting again after a failure, so that a persistent one does not become a busy loop.
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);

/// Accept connections until the process is asked to terminate.
///
/// Requests already in progress finish and send their responses before this function returns.
pub(crate) async fn serve(uds: UnixListener, app: Router) -> anyhow::Result<()> {
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    serve_with_shutdown(uds, app, async move {
        tokio::select! {
            _ = sigint.recv() => (),
            _ = sigterm.recv() => (),
        }
    })
    .await;
    Ok(())
}

async fn serve_with_shutdown(uds: UnixListener, app: Router, shutdown: impl Future<Output = ()>) {
    let graceful = GracefulShutdown::new();
    tokio::pin!(shutdown);

    loop {
        let socket = tokio::select! {
            result = uds.accept() => match result {
                Ok((socket, _remote_addr)) => socket,
                Err(error) => {
                    // running out of file descriptors is usually temporary, so keep the server alive
                    tracing::error!("failed to accept a connection: {error}");

                    tokio::time::sleep(ACCEPT_RETRY_DELAY).await;

                    continue;
                },
            },
            _ = &mut shutdown => break,
        };

        let tower_service = app.clone();
        let watcher = graceful.watcher();

        tokio::spawn(async move {
            let socket = TokioIo::new(socket);

            let hyper_service = hyper::service::service_fn(move |request: Request<Incoming>| {
                tower_service.clone().call(request)
            });

            let builder = server::conn::auto::Builder::new(TokioExecutor::new());
            let connection = builder.serve_connection(socket, hyper_service);
            if let Err(error) = watcher.watch(connection).await {
                tracing::error!("failed to serve connection: {error:#}");
            }
        });
    }

    drop(uds);
    graceful.shutdown().await;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::routing::get;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixStream,
        sync::{Notify, oneshot},
    };

    use super::*;

    #[test]
    fn shutdown_waits_for_responses_and_closes_idle_connections() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("service.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let started = Arc::new(Notify::new());
            let finish = Arc::new(Notify::new());
            let handler_started = started.clone();
            let handler_finish = finish.clone();
            let app = Router::new().route(
                "/",
                get(move || {
                    let started = handler_started.clone();
                    let finish = handler_finish.clone();
                    async move {
                        started.notify_one();
                        finish.notified().await;
                        "Purge finished."
                    }
                }),
            );
            let (shutdown_tx, shutdown_rx) = oneshot::channel();
            let mut server = tokio::spawn(serve_with_shutdown(listener, app, async {
                shutdown_rx.await.unwrap();
            }));
            let mut idle = UnixStream::connect(&path).await.unwrap();
            let mut client = UnixStream::connect(&path).await.unwrap();
            client.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
            tokio::time::timeout(Duration::from_secs(5), started.notified()).await.unwrap();
            shutdown_tx.send(()).unwrap();
            assert!(tokio::time::timeout(Duration::from_millis(20), &mut server).await.is_err());
            finish.notify_one();

            let mut response = String::new();
            tokio::time::timeout(Duration::from_secs(5), client.read_to_string(&mut response))
                .await
                .unwrap()
                .unwrap();
            assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
            assert!(response.ends_with("Purge finished."));
            tokio::time::timeout(Duration::from_secs(5), server).await.unwrap().unwrap();
            assert_eq!(
                0,
                tokio::time::timeout(Duration::from_secs(5), idle.read(&mut [0]))
                    .await
                    .unwrap()
                    .unwrap()
            );
        });
    }
}
