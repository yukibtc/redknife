use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use chrono::{DateTime, Local};
use tokio::io::{AsyncWriteExt, Stdout};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::{io, signal};

use crate::ftp::{FtpServer, FtpServerEvent, FtpServerHandler};
use crate::hexdump::HexDump;

pub(super) async fn run(ip: IpAddr, port: u16, raw: bool) -> io::Result<()> {
    let addr: SocketAddr = SocketAddr::new(ip, port);

    let listener: TcpListener = TcpListener::bind(addr).await?;
    let local_addr: SocketAddr = listener.local_addr()?;

    let mut stdout: Stdout = io::stdout();

    stdout.write_all(b"FTP server listening on ").await?;
    stdout.write_all(local_addr.to_string().as_bytes()).await?;
    stdout.write_all(b"\nCtrl+C to stop.\n\n").await?;
    stdout.flush().await?;

    let (tx, rx) = mpsc::unbounded_channel();

    let handler: Arc<dyn FtpServerHandler> = Arc::new(Handler { tx });
    let server = FtpServer::new(listener, handler);

    tokio::select! {
       result = server.run(signal::ctrl_c()) => {
            result?;
        }
        result = writer(&mut stdout, rx, raw) => {
            result?;
        }
    }

    stdout.write_all(b"\nListener stopped.\n").await?;
    stdout.flush().await?;

    Ok(())
}

struct Handler {
    tx: mpsc::UnboundedSender<(SocketAddr, FtpServerEvent)>,
}

impl FtpServerHandler for Handler {
    fn handle(&self, client: SocketAddr, event: FtpServerEvent) {
        let _ = self.tx.send((client, event));
    }
}

async fn writer(
    stdout: &mut Stdout,
    mut rx: mpsc::UnboundedReceiver<(SocketAddr, FtpServerEvent)>,
    raw: bool,
) -> io::Result<()> {
    while let Some((client, event)) = rx.recv().await {
        // Skip raw events if not in raw mode
        if event.is_raw() && !raw {
            continue;
        }

        let timestamp: DateTime<Local> = Local::now();

        let mut output: Vec<u8> = format!(
            "[{}] Event from {client}\n",
            timestamp.format("%Y-%m-%d %H:%M:%S"),
        )
        .into_bytes();

        match event {
            FtpServerEvent::Raw(bytes) => {
                let dump = HexDump::new(&bytes);
                output.extend(dump.to_string().into_bytes());
                output.extend_from_slice(b"\n\n");
            }
            FtpServerEvent::ReceivedCredentials { username, password } => {
                output.extend_from_slice(b"Credentials:\n");
                output.extend_from_slice(b" - Username: ");
                output.extend_from_slice(username.as_bytes());
                output.extend_from_slice(b"\n");
                output.extend_from_slice(b" - Password: ");
                output.extend_from_slice(password.as_bytes());
                output.extend_from_slice(b"\n\n");
            }
            FtpServerEvent::Disconnected => {
                output.extend_from_slice(b"Disconnected\n\n");
            }
        }

        stdout.write_all(&output).await?;
        stdout.flush().await?;
    }

    Ok(())
}
