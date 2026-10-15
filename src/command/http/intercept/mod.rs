use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::mpsc;

mod app;
mod tui;
mod ui;

use crate::http::intercept::HttpInterceptHandler;
use crate::proxy::http_forward_server::HttpForwardProxyServer;

pub async fn run(ip: IpAddr, port: u16, out_dir: Option<PathBuf>) -> io::Result<()> {
    let addr: SocketAddr = SocketAddr::new(ip, port);

    let listener: TcpListener = TcpListener::bind(addr).await?;
    let local_addr: SocketAddr = listener.local_addr()?;

    let (sender, receiver) = mpsc::channel(4096);

    let handler = Arc::new(HttpInterceptHandler::new(sender));
    let proxy = HttpForwardProxyServer::new(listener, handler);

    tokio::select! {
        res = proxy.run() => {
            res?;
        }
        res = tui::run(receiver, local_addr, out_dir) => {
            res?;
        }
    }

    Ok(())
}
