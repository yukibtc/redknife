use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use bytes::Bytes;
use chrono::{DateTime, Local};
use http::{Request, Uri};
use tokio::fs::File;
use tokio::io::{self, AsyncWriteExt, Stdout};
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::mpsc;

pub(super) mod dump;

use self::dump::HttpProxyDumpFormat;
use crate::http::proxy::{HttpProxyServer, PendingRequest};

pub(in crate::cli) async fn run(
    ip: IpAddr,
    port: u16,
    dump: Option<PathBuf>,
    dump_format: HttpProxyDumpFormat,
) -> io::Result<()> {
    let addr: SocketAddr = SocketAddr::new(ip, port);

    let listener: TcpListener = TcpListener::bind(addr).await?;

    // TODO: write these to stderr
    println!("HTTP proxy listening on {}", listener.local_addr()?);
    println!("Ctrl+C to stop.\n");

    let (sender, receiver) = mpsc::channel(4096);

    // Create the HTTP proxy server
    let proxy: HttpProxyServer = HttpProxyServer::new(listener, sender);

    tokio::select! {
        result = proxy.run() => result?,
        result = writer(receiver, dump, dump_format) => result?,
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

impl From<PendingRequest> for Output {
    fn from(request: PendingRequest) -> Self {
        Self {
            id: request.id,
            timestamp: request.timestamp,
            req: request.original,
            raw: request.raw,
            uri: request.original_uri,
        }
    }
}

async fn writer(
    mut rx: mpsc::Receiver<PendingRequest>,
    dump: Option<PathBuf>,
    dump_format: HttpProxyDumpFormat,
) -> io::Result<()> {
    let mut stdout: Stdout = io::stdout();
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
