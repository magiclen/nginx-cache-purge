use std::time::Duration;

use axum::{Router, extract::Request};
use hyper::body::Incoming;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server,
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
/// A purge which has already started still runs to its end, because dropping the tokio runtime waits for the blocking task it runs in.
pub(crate) async fn serve(uds: UnixListener, app: Router) -> anyhow::Result<()> {
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

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
            _ = sigint.recv() => break,
            _ = sigterm.recv() => break,
        };

        let tower_service = app.clone();

        tokio::spawn(async move {
            let socket = TokioIo::new(socket);

            let hyper_service = hyper::service::service_fn(move |request: Request<Incoming>| {
                tower_service.clone().call(request)
            });

            if let Err(error) = server::conn::auto::Builder::new(TokioExecutor::new())
                .serve_connection(socket, hyper_service)
                .await
            {
                tracing::error!("failed to serve connection: {error:#}");
            }
        });
    }

    Ok(())
}
