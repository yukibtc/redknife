use std::net::SocketAddr;

use tokio::net::{TcpListener, TcpStream, lookup_host};
use tokio::{io, signal};

use crate::host_and_port::HostAndPort;
use crate::{logger, socks};

pub(super) async fn run(
    listen: SocketAddr,
    target: HostAndPort,
    socks: Option<SocketAddr>,
) -> io::Result<()> {
    logger::init()?;

    let listener: TcpListener = TcpListener::bind(listen).await?;
    let local_addr: SocketAddr = listener.local_addr()?;

    match socks {
        Some(proxy) => {
            tracing::info!(
                "TCP forwarder listening on {local_addr} -> {target} (via SOCKS5 {proxy})"
            );
        }
        None => {
            tracing::info!("TCP forwarder listening on {local_addr} -> {target}");
        }
    }
    tracing::info!("Ctrl+C to stop.");

    tokio::select! {
        result = accept_loop(listener, target, socks) => {
            result?;
        }
        _ = signal::ctrl_c() => {}
    }

    tracing::info!("Forwarder stopped.");

    Ok(())
}

/// Accept incoming connections and forward each one to the target.
async fn accept_loop(
    listener: TcpListener,
    target: HostAndPort,
    socks: Option<SocketAddr>,
) -> io::Result<()> {
    loop {
        let (client, peer): (TcpStream, SocketAddr) = listener.accept().await?;

        tracing::info!("New connection from {peer}");

        let target = target.clone();

        tokio::spawn(async move {
            match relay(client, &target, socks).await {
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
async fn relay(
    client: TcpStream,
    target: &HostAndPort,
    socks: Option<SocketAddr>,
) -> io::Result<(u64, u64)> {
    let mut upstream: TcpStream = match socks {
        Some(proxy) => socks::connect(proxy, target).await?,
        None => {
            let addr: SocketAddr = resolve(target).await?;

            TcpStream::connect(addr).await.map_err(|err| {
                io::Error::new(err.kind(), format!("cannot connect to {target}: {err}"))
            })?
        }
    };
    let mut client: TcpStream = client;

    io::copy_bidirectional(&mut client, &mut upstream).await
}

/// Resolve a target whose host may be a hostname.
async fn resolve(target: &HostAndPort) -> io::Result<SocketAddr> {
    lookup_host((target.host(), target.port()))
        .await?
        .next()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("failed to resolve target address: {target}"),
            )
        })
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use crate::socks::SocksListener;

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

        tokio::spawn(accept_loop(listener, HostAndPort::from(target), None));

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
    async fn relays_traffic_through_a_socks_proxy() {
        let target: SocketAddr = spawn_echo_server().await;

        // A fake tunnel: a SOCKS listener whose accepted connections are
        // relayed to the origin.
        let proxy_listener: SocksListener = SocksListener::bind("127.0.0.1:0").await.unwrap();
        let proxy: SocketAddr = proxy_listener.local_addr().unwrap();

        tokio::spawn(async move {
            while let Ok((stream, remote)) = proxy_listener.accept().await {
                tokio::spawn(async move {
                    let mut upstream: TcpStream =
                        TcpStream::connect((remote.host(), remote.port()))
                            .await
                            .unwrap();
                    let mut stream = stream;
                    let _ = io::copy_bidirectional(&mut stream, &mut upstream).await;
                });
            }
        });

        let listener: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen: SocketAddr = listener.local_addr().unwrap();

        tokio::spawn(accept_loop(
            listener,
            HostAndPort::from(target),
            Some(proxy),
        ));

        let mut client: TcpStream = TcpStream::connect(listen).await.unwrap();

        client.write_all(b"ping").await.unwrap();
        let mut buffer: [u8; 4] = [0; 4];
        client.read_exact(&mut buffer).await.unwrap();
        assert_eq!(&buffer, b"ping");

        client.write_all(b"pongpong").await.unwrap();
        let mut buffer: [u8; 8] = [0; 8];
        client.read_exact(&mut buffer).await.unwrap();
        assert_eq!(&buffer, b"pongpong");
    }

    #[tokio::test]
    async fn resolves_target_host() {
        let target: HostAndPort = "127.0.0.1:9999".parse().unwrap();

        let resolved: SocketAddr = resolve(&target).await.unwrap();
        assert_eq!(resolved.to_string(), "127.0.0.1:9999");
    }

    #[tokio::test]
    async fn fails_to_resolve_unknown_host() {
        // The .invalid TLD is guaranteed not to resolve (RFC 2606).
        let target: HostAndPort = "nonexistent.invalid:9999".parse().unwrap();

        assert!(resolve(&target).await.is_err());
    }
}
