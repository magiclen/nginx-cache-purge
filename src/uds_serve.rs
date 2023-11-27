use axum::{extract::Request, Router};
use hyper::body::Incoming;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server,
};
use tokio::net::UnixListener;
use tower::Service;

pub(crate) async fn serve(uds: UnixListener, app: Router) -> anyhow::Result<()> {
    loop {
        let (socket, _remote_addr) = uds.accept().await?;

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
                eprintln!("failed to serve connection: {error:#}");
            }
        });
    }
}
