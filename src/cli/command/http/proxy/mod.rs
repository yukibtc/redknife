use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use chrono::{DateTime, Local};
use http::{Request, Uri};
use tokio::fs::File;
use tokio::io::{self, AsyncWriteExt, Stderr, Stdout};
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::mpsc;

pub(super) mod dump;
mod middleware;

use self::dump::HttpProxyDumpFormat;
use self::middleware::{HttpProxyServerMiddleware, MiddlewareRequest};
use crate::http::proxy::{HttpProxyServer, HttpProxyServerHandler};

pub(in crate::cli) async fn run(
    ip: IpAddr,
    port: u16,
    dump: Option<PathBuf>,
    dump_format: HttpProxyDumpFormat,
) -> io::Result<()> {
    let addr: SocketAddr = SocketAddr::new(ip, port);

    let listener: TcpListener = TcpListener::bind(addr).await?;

    let mut stdout: Stdout = io::stdout();
    let mut stderr: Stderr = io::stderr();

    stderr.write_all(b"HTTP proxy listening on ").await?;
    stderr
        .write_all(listener.local_addr()?.to_string().as_bytes())
        .await?;
    stderr.write_all(b"\nCtrl+C to stop.\n\n").await?;
    stderr.flush().await?;

    let (sender, receiver) = mpsc::unbounded_channel();

    let middleware: HttpProxyServerMiddleware = HttpProxyServerMiddleware::new(sender);

    // Create the HTTP proxy server
    let handler: Arc<dyn HttpProxyServerHandler> = Arc::new(middleware);
    let proxy: HttpProxyServer = HttpProxyServer::new(listener, handler);

    tokio::select! {
        result = proxy.run() => result?,
        result = writer(&mut stdout, receiver, dump, dump_format) => result?,
        result = signal::ctrl_c() => result?,
    }

    Ok(())
}

struct Output {
    id: u64,
    timestamp: DateTime<Local>,
    req: Request<Bytes>,
    raw: Bytes,
    uri: Uri,
}

impl fmt::Display for Output {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] #{} {} {} ({} bytes)",
            self.timestamp.format("%Y-%m-%d %H:%M:%S"),
            self.id,
            self.req.method(),
            self.uri,
            self.raw.len()
        )
    }
}

impl From<MiddlewareRequest> for Output {
    fn from(request: MiddlewareRequest) -> Self {
        Self {
            id: request.id,
            timestamp: request.timestamp,
            req: request.req,
            raw: request.raw,
            uri: request.uri,
        }
    }
}

async fn writer(
    stdout: &mut Stdout,
    mut rx: mpsc::UnboundedReceiver<MiddlewareRequest>,
    dump: Option<PathBuf>,
    dump_format: HttpProxyDumpFormat,
) -> io::Result<()> {
    let mut file: Option<File> = match dump {
        Some(path) => Some(
            File::options()
                .write(true)
                .create(true)
                .append(true)
                .open(path)
                .await?,
        ),
        None => None,
    };

    while let Some(request) = rx.recv().await {
        // Dump to file
        if let Some(file) = file.as_mut() {
            dump_format.write(&request, file).await?;
            file.flush().await?;
        }

        // Write to stdout
        let output: Output = request.into();
        let output: String = output.to_string();
        stdout.write_all(output.as_bytes()).await?;
        stdout.write_u8(b'\n').await?;
        stdout.flush().await?;
    }

    Ok(())
}
