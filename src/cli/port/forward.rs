use std::net::SocketAddr;

use tokio::net::{TcpListener, TcpStream};
use tokio::{io, signal};

use super::resolve_target;
use crate::logger;

pub(super) async fn run(listen: SocketAddr, target: String) -> io::Result<()> {
    logger::init()?;

    let target: SocketAddr = resolve_target(&target).await?;

    let listener: TcpListener = TcpListener::bind(listen).await?;
    let local_addr: SocketAddr = listener.local_addr()?;

    tracing::info!("TCP forwarder listening on {local_addr} -> {target}");
    tracing::info!("Ctrl+C to stop.");

    tokio::select! {
        result = accept_loop(listener, target) => {
            result?;
        }
        _ = signal::ctrl_c() => {}
    }

    tracing::info!("Forwarder stopped.");

    Ok(())
}

/// Accept incoming connections and forward each one to the target.
async fn accept_loop(listener: TcpListener, target: SocketAddr) -> io::Result<()> {
    loop {
        let (client, peer): (TcpStream, SocketAddr) = listener.accept().await?;

        tracing::info!("New connection from {peer}");

        tokio::spawn(async move {
            match relay(client, target).await {
                Ok((sent, received)) => {
                    tracing::info!(
                        "Connection {peer} -> {target} closed ({sent} bytes sent, {received} bytes received)",
                    );
                }
                Err(err) => {
                    tracing::warn!("Connection {peer} -> {target} failed: {err}");
                }
            }
        });
    }
}

/// Pipe a client connection to the target in both directions.
///
/// Returns the number of bytes sent to the target and received from it.
async fn relay(client: TcpStream, target: SocketAddr) -> io::Result<(u64, u64)> {
    let mut upstream: TcpStream = TcpStream::connect(target)
        .await
        .map_err(|err| io::Error::new(err.kind(), format!("cannot connect to {target}: {err}")))?;
    let mut client: TcpStream = client;

    io::copy_bidirectional(&mut client, &mut upstream).await
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// Start a fake remote service which echoes back every byte received.
    async fn spawn_echo_server() -> SocketAddr {
        let listener: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(echo(socket));
            }
        });

        addr
    }

    async fn echo(socket: TcpStream) {
        let (mut reader, mut writer) = socket.into_split();
        let _ = io::copy(&mut reader, &mut writer).await;
    }

    #[tokio::test]
    async fn relays_traffic_in_both_directions() {
        let target: SocketAddr = spawn_echo_server().await;

        let listener: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen: SocketAddr = listener.local_addr().unwrap();

        tokio::spawn(accept_loop(listener, target));

        let mut client: TcpStream = TcpStream::connect(listen).await.unwrap();

        // Client -> forwarder -> target -> forwarder -> client (echo)
        client.write_all(b"ping").await.unwrap();
        let mut buffer: [u8; 4] = [0; 4];
        client.read_exact(&mut buffer).await.unwrap();
        assert_eq!(&buffer, b"ping");

        // A second round-trip over the same relayed connection
        client.write_all(b"pongpong").await.unwrap();
        let mut buffer: [u8; 8] = [0; 8];
        client.read_exact(&mut buffer).await.unwrap();
        assert_eq!(&buffer, b"pongpong");
    }

    #[tokio::test]
    async fn resolves_socket_address_target() {
        let target: SocketAddr = resolve_target("127.0.0.1:9999").await.unwrap();
        assert_eq!(target.to_string(), "127.0.0.1:9999");
    }

    #[tokio::test]
    async fn rejects_target_without_port() {
        let result: io::Result<SocketAddr> = resolve_target("localhost").await;
        assert!(result.is_err());
    }
}
