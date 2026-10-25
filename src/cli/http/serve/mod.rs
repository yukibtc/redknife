use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::{io, path};

use tokio::net::TcpListener;
use tokio::signal;

mod handler;

use self::handler::Service;
use crate::http::server::{HttpServer, HttpServerHandler};
use crate::logger;

pub(super) async fn run(ip: IpAddr, port: u16, dir: PathBuf, upload: bool) -> io::Result<()> {
    logger::init()?;

    let addr: SocketAddr = SocketAddr::new(ip, port);

    let listener: TcpListener = TcpListener::bind(addr).await?;
    let local_addr: SocketAddr = listener.local_addr()?;

    let dir: PathBuf = path::absolute(&dir)?;
    let upload_dir: Option<PathBuf> = if upload {
        Some(dir.join("uploads"))
    } else {
        None
    };

    tracing::info!("HTTP Server listening on http://{local_addr}/");
    tracing::info!("Serving files from '{}'", dir.display());

    if let Some(upload_dir) = &upload_dir {
        let command: &'static str = "curl -X POST --data-binary \"@<path-to-file>\" -H \"X-Filename: <file-name>\" http://<IP>:<PORT>/upload";
        tracing::info!("Upload files using '{}'", command);
        tracing::info!("Files will be uploaded at '{}'", upload_dir.display());
    }

    tracing::info!("Ctrl+C to stop.");

    let service: Service = Service::new(dir, upload_dir);
    let handler: Arc<dyn HttpServerHandler> = Arc::new(service);
    let server: HttpServer = HttpServer::new(listener, handler);

    server.run(signal::ctrl_c()).await?;

    Ok(())
}
