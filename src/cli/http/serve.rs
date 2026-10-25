use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::{io, path};

use tokio::net::TcpListener;
use tokio::signal;

use crate::http::server::HttpServer;
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

    let server: HttpServer = HttpServer::new(listener, dir, upload_dir);

    tracing::info!("Ctrl+C to stop.");

    server.run(signal::ctrl_c()).await?;

    Ok(())
}
