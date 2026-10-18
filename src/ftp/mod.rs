use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use tokio::io::{self, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;

mod packet;
mod response;
mod session;

use self::packet::FtpPacket;
use self::response::FtpResponse;
use self::session::{FtpSession, FtpSessionLogin};

pub enum FtpServerEvent {
    Raw(Bytes),
    ReceivedCredentials { username: String, password: String },
    Disconnected,
}

impl FtpServerEvent {
    #[inline]
    pub fn is_raw(&self) -> bool {
        matches!(self, Self::Raw(_))
    }
}

pub trait FtpServerHandler: Send + Sync {
    fn handle(&self, client: SocketAddr, event: FtpServerEvent);
}

pub struct FtpServer {
    listener: TcpListener,
    handler: Arc<dyn FtpServerHandler>,
}

impl FtpServer {
    #[inline]
    pub fn new(listener: TcpListener, handler: Arc<dyn FtpServerHandler>) -> Self {
        Self { listener, handler }
    }

    pub async fn run<F>(self, shutdown: F) -> io::Result<()>
    where
        F: Future<Output = io::Result<()>>,
    {
        let mut connections: JoinSet<()> = JoinSet::new();

        tokio::pin!(shutdown);

        loop {
            tokio::select! {
                biased;
                result = &mut shutdown => {
                    result?;
                    break;
                }
                result = self.listener.accept() => {
                    let (stream, client) = result?;

                    let handler: Arc<dyn FtpServerHandler> = self.handler.clone();

                    connections.spawn(async move {
                        let _ = handle(stream, client, handler).await;
                    });
                }

                Some(result) = connections.join_next(), if !connections.is_empty() => {
                    if let Err(error) = result {
                        eprintln!("Connection task error: {error}");
                    }
                }
            }
        }

        connections.shutdown().await;

        Ok(())
    }
}

async fn handle(
    mut stream: TcpStream,
    client: SocketAddr,
    handler: Arc<dyn FtpServerHandler>,
) -> io::Result<()> {
    let (rx, mut tx) = stream.split();

    let mut reader = BufReader::new(rx);
    let mut session: FtpSession = FtpSession::new();

    // Banner FTP
    send_packet(&mut tx, FtpPacket::welcome()).await?;

    loop {
        let mut data = Vec::new();

        // Read an FTP line terminated by \n
        if reader.read_until(b'\n', &mut data).await? == 0 {
            break;
        }

        handler.handle(client, FtpServerEvent::Raw(Bytes::copy_from_slice(&data)));

        let Ok(response) = FtpResponse::parse(data) else {
            send_packet(&mut tx, FtpPacket::command_not_implemented()).await?;
            continue;
        };

        match response {
            FtpResponse::User(user) => {
                if user.is_empty() {
                    send_packet(&mut tx, FtpPacket::syntax_error()).await?;
                    continue;
                }

                session.login = FtpSessionLogin::PendingPassword { username: user };

                send_packet(&mut tx, FtpPacket::password_required()).await?;
            }
            FtpResponse::Pass(password) => {
                // Received PASS before USER -> bad sequence
                let Some(username) = session.login.username().cloned() else {
                    send_packet(&mut tx, FtpPacket::bad_sequence()).await?;
                    continue;
                };

                if password.is_empty() {
                    send_packet(&mut tx, FtpPacket::syntax_error()).await?;
                    continue;
                }

                session.login = FtpSessionLogin::LoggedIn {
                    username: username.clone(),
                };

                handler.handle(
                    client,
                    FtpServerEvent::ReceivedCredentials { username, password },
                );

                send_packet(&mut tx, FtpPacket::login_successful()).await?;
            }
            FtpResponse::Syst => {
                send_packet(&mut tx, FtpPacket::syst()).await?;
            }
            FtpResponse::Noop => {
                send_packet(&mut tx, FtpPacket::noop()).await?;
            }
            FtpResponse::Quit => {
                send_packet(&mut tx, FtpPacket::quit()).await?;
                break;
            }
        }
    }

    handler.handle(client, FtpServerEvent::Disconnected);

    Ok(())
}

#[inline]
async fn send_packet<W>(stream: &mut W, packet: FtpPacket) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    stream.write_all(&packet.to_bytes()).await
}
