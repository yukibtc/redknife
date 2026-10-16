use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ratatui::DefaultTerminal;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::mpsc;

use crate::http::proxy::{HttpProxyServer, ProxyState};
use crate::tui::http::editor::{self, ExitLabel};
use crate::tui::http::proxy::{Action, HttpProxyTuiSession};
use crate::tui::terminal::TerminalSession;

pub async fn run(ip: IpAddr, port: u16, out_dir: Option<PathBuf>) -> io::Result<()> {
    let addr: SocketAddr = SocketAddr::new(ip, port);

    let listener: TcpListener = TcpListener::bind(addr).await?;
    let listen_addr: SocketAddr = listener.local_addr()?;

    let (sender, receiver) = mpsc::channel(4096);

    // Create the HTTP proxy state, that will be shared between the interceptor and the TUI session
    let state: Arc<ProxyState> = Arc::new(ProxyState::default());

    // Create the HTTP proxy server
    let proxy: HttpProxyServer = HttpProxyServer::new(listener, sender, Arc::clone(&state));

    // Keep one terminal session while switching between the proxy and editor.
    let mut terminal: TerminalSession = TerminalSession::new()?;
    let mut proxy_tui_session: HttpProxyTuiSession =
        HttpProxyTuiSession::new(receiver, listen_addr, state);

    tokio::select! {
        result = proxy.run() => result?,
        result = run_tui(&mut proxy_tui_session, terminal.terminal(), out_dir.as_deref()) => result?,
        result = signal::ctrl_c() => result?,
    }

    Ok(())
}

async fn run_tui(
    proxy_tui_session: &mut HttpProxyTuiSession,
    terminal: &mut DefaultTerminal,
    directory: Option<&Path>,
) -> io::Result<()> {
    loop {
        match proxy_tui_session.run(terminal, directory).await? {
            Action::Edit { id, raw, target } => {
                if let Some(edited) = editor::edit(terminal, raw, &target, directory).await? {
                    proxy_tui_session.apply_edit(id, edited);
                }
            }
            Action::Repeat { raw, target } => {
                editor::repeat(terminal, raw, &target, directory, ExitLabel::Back).await?;
            }
            Action::Quit => return Ok(()),
        }
    }
}
