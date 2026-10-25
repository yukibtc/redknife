use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::fs::File;
use tokio::io::{self, AsyncWriteExt, Stderr, Stdout};
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::mpsc;

mod format;
mod middleware;
mod rules;

pub(super) use self::format::HttpProxyFormat;
use self::middleware::{HttpProxyServerMiddleware, MiddlewareEvent};
use self::rules::loader::RulesLoader;
use crate::http::proxy::HttpProxyService;
use crate::http::server::{HttpServer, HttpServerHandler};

pub(super) async fn rules_example() -> io::Result<()> {
    let mut stdout: Stdout = io::stdout();
    stdout.write_all(rules::EXAMPLE.as_bytes()).await?;
    stdout.flush().await
}

pub(super) async fn run(
    ip: IpAddr,
    port: u16,
    rules: Option<PathBuf>,
    watch_rules: bool,
    stdout_format: HttpProxyFormat,
    dump: Option<PathBuf>,
    dump_format: HttpProxyFormat,
) -> io::Result<()> {
    let addr: SocketAddr = SocketAddr::new(ip, port);

    let (mut reloader, rules) = RulesLoader::new(rules, watch_rules).await?;

    let listener: TcpListener = TcpListener::bind(addr).await?;

    let mut stdout: Stdout = io::stdout();
    let mut stderr: Stderr = io::stderr();

    stderr.write_all(b"HTTP Proxy listening on ").await?;
    stderr
        .write_all(listener.local_addr()?.to_string().as_bytes())
        .await?;
    stderr.write_all(b"\nCtrl+C to stop.\n\n").await?;
    stderr.flush().await?;

    let (sender, receiver) = mpsc::unbounded_channel();

    let middleware: HttpProxyServerMiddleware = HttpProxyServerMiddleware::new(sender, rules);

    // Create the HTTP proxy server
    let proxy: HttpProxyService<HttpProxyServerMiddleware> = HttpProxyService::new(middleware);
    let handler: Arc<dyn HttpServerHandler> = Arc::new(proxy);
    let server = HttpServer::new(listener, handler);

    tokio::select! {
        result = server.run(signal::ctrl_c()) => result?,
        result = writer(&mut stdout, receiver, stdout_format, dump, dump_format) => result?,
        result = reloader.run(&mut stderr) => result?,
    }

    Ok(())
}

async fn writer(
    stdout: &mut Stdout,
    mut rx: mpsc::UnboundedReceiver<MiddlewareEvent>,
    stdout_format: HttpProxyFormat,
    dump: Option<PathBuf>,
    dump_format: HttpProxyFormat,
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
        stdout_format.write(&request, stdout).await?;
        stdout.flush().await?;
    }

    Ok(())
}
