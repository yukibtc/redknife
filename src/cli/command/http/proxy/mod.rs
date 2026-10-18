use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::fs::{self, File};
use tokio::io::{self, AsyncWriteExt, Stderr, Stdout};
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::mpsc;

pub(super) mod dump;
mod middleware;
mod rules;

use self::dump::HttpProxyDumpFormat;
use self::middleware::{HttpProxyServerMiddleware, MiddlewareEvent};
use self::rules::{RuleOutcome, Rules};
use crate::http::proxy::{HttpProxyServer, HttpProxyServerHandler};

pub(in crate::cli) async fn run(
    ip: IpAddr,
    port: u16,
    rules: Option<PathBuf>,
    dump: Option<PathBuf>,
    dump_format: HttpProxyDumpFormat,
) -> io::Result<()> {
    let addr: SocketAddr = SocketAddr::new(ip, port);

    let rules: Rules = match rules {
        Some(path) => {
            let content: String = fs::read_to_string(path).await?;
            toml::from_str(&content)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?
        }
        None => Rules::default(),
    };

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

    let middleware: HttpProxyServerMiddleware = HttpProxyServerMiddleware::new(sender, rules);

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

struct Output<'a> {
    event: &'a MiddlewareEvent,
}

impl<'a> From<&'a MiddlewareEvent> for Output<'a> {
    fn from(event: &'a MiddlewareEvent) -> Self {
        Self { event }
    }
}

impl fmt::Display for Output<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let event = self.event;

        write!(
            f,
            "[{}] #{} {} {} ",
            event.timestamp.format("%Y-%m-%d %H:%M:%S"),
            event.id,
            event.original.method(),
            event.uri,
        )?;

        match event.report.outcome {
            RuleOutcome::Continue => write!(f, "ALLOW")?,
            RuleOutcome::Deny(code) => {
                write!(f, "DENY {}", code.as_u16())?;
            }
        }

        match &event.modified_raw {
            Some(modified) => write!(
                f,
                " modified ({} -> {} bytes)",
                event.original_raw.len(),
                modified.len(),
            ),
            None => write!(f, " ({} bytes)", event.original_raw.len()),
        }?;

        if !event.report.matched_rules.is_empty() {
            write!(f, " rules={:?}", event.report.matched_rules)?;
        }

        Ok(())
    }
}

async fn writer(
    stdout: &mut Stdout,
    mut rx: mpsc::UnboundedReceiver<MiddlewareEvent>,
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
        let output: Output = (&request).into();
        let output: String = output.to_string();
        stdout.write_all(output.as_bytes()).await?;
        stdout.write_u8(b'\n').await?;
        stdout.flush().await?;
    }

    Ok(())
}
