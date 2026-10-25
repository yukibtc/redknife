use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::signal;

mod handler;

use self::handler::Service;
use crate::http::server::{HttpServer, HttpServerHandler};
use crate::logger;

pub async fn run(ip: IpAddr, port: u16) -> io::Result<()> {
    logger::init()?;

    let addr: SocketAddr = SocketAddr::new(ip, port);

    let listener: TcpListener = TcpListener::bind(addr).await?;

    tracing::info!(
        "HTTP Callback listening on http://{}/",
        listener.local_addr()?
    );
    tracing::info!("Ctrl+C to stop.");

    let handler: Arc<dyn HttpServerHandler> = Arc::new(Service);
    let server: HttpServer = HttpServer::new(listener, handler);

    server.run(signal::ctrl_c()).await?;

    tracing::info!("Listener stopped.");

    Ok(())
}
