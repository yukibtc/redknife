use std::future::poll_fn;
use std::io::{self, ErrorKind};
use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use h2::{RecvStream, SendStream};
use http::HeaderMap;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::{net, time};

mod connector;
mod listener;
mod tls;

pub use self::connector::TunnelConnector;
pub use self::listener::{TunnelListener, Waiting};
pub use self::tls::{TunnelTlsCerts, TunnelTlsFingerprint};
use crate::host_and_port::HostAndPort;

// ALPN protocol identifier, acting as the wire magic and version.
const ALPN: &[u8] = b"rk/1";

const DIAL_PATH: &str = "/dial";
const OPDIAL_PATH: &str = "/opdial";

const HDR_TARGET: &str = "x-rk-target";
const HDR_PORT: &str = "x-rk-port";
const HDR_AUTHORIZATION: &str = "authorization";

const DIAL_TIMEOUT: Duration = Duration::from_secs(3);

#[inline]
fn h2_to_io_err(error: h2::Error) -> io::Error {
    io::Error::new(ErrorKind::ConnectionReset, error)
}

fn extract_hdr_target(headers: &HeaderMap) -> io::Result<HostAndPort> {
    headers
        .get(HDR_TARGET)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "missing or invalid target header"))
}

// Bridge a dialed TCP connection with an HTTP/2 stream.
async fn bridge(recv: RecvStream, send: SendStream<Bytes>, stream: TcpStream) -> io::Result<()> {
    let (mut tcp_read, mut tcp_write) = stream.into_split();

    let upstream = pump_send(send, &mut tcp_read);
    let downstream = pump_recv(recv, &mut tcp_write);

    tokio::try_join!(upstream, downstream)?;

    Ok(())
}

async fn resolve_target(target: &HostAndPort) -> io::Result<SocketAddr> {
    net::lookup_host((target.host(), target.port()))
        .await
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("cannot resolve {}: {error}", target.host()),
            )
        })?
        .next()
        .ok_or_else(|| {
            io::Error::new(
                ErrorKind::NotFound,
                format!("cannot resolve {}", target.host()),
            )
        })
}

async fn dial(addr: SocketAddr) -> io::Result<TcpStream> {
    let stream: TcpStream = time::timeout(DIAL_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| io::Error::new(ErrorKind::TimedOut, format!("dial {addr} timed out")))??;
    stream.set_nodelay(true)?;

    Ok(stream)
}

// Pump bytes from a reader into an HTTP/2 send stream, respecting the peer flow-control window.
async fn pump_send<R>(mut send: SendStream<Bytes>, read: &mut R) -> io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut buf: [u8; 16 * 1024] = [0; 16 * 1024];

    loop {
        let read: usize = read.read(&mut buf).await?;
        if read == 0 {
            // EOF: signal the end of the upstream direction.
            return send.send_data(Bytes::new(), true).map_err(h2_to_io_err);
        }

        let mut chunk: Bytes = Bytes::copy_from_slice(&buf[..read]);

        while !chunk.is_empty() {
            send.reserve_capacity(chunk.len());

            let granted: usize = poll_fn(|cx| send.poll_capacity(cx))
                .await
                .ok_or_else(|| io::Error::new(ErrorKind::BrokenPipe, "send stream is closed"))?
                .map_err(h2_to_io_err)?;

            send.send_data(chunk.split_to(granted.min(chunk.len())), false)
                .map_err(h2_to_io_err)?;
        }
    }
}

// Pump bytes from an HTTP/2 receive stream into a writer, releasing the flow-control capacity as data is consumed.
async fn pump_recv<W>(mut recv: RecvStream, write: &mut W) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    while let Some(chunk) = recv.data().await {
        let chunk: Bytes = chunk.map_err(h2_to_io_err)?;
        recv.flow_control()
            .release_capacity(chunk.len())
            .map_err(h2_to_io_err)?;
        write.write_all(&chunk).await?;
    }

    // End of the downstream direction: propagate the half-close to the writer.
    write.shutdown().await
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::mpsc;
    use tokio::time;

    use super::*;
    use crate::host_and_port::HostAndPort;

    const TOKEN: &str = "secret";

    async fn spawn_echo() -> SocketAddr {
        let listener: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (mut read, mut write) = socket.into_split();
                    let _ = tokio::io::copy(&mut read, &mut write).await;
                });
            }
        });
        addr
    }

    async fn spawn_operator(
        certs: &TunnelTlsCerts,
        reverse_ports: impl IntoIterator<Item = u16>,
    ) -> (mpsc::Sender<Waiting>, SocketAddr) {
        let (pending_tx, pending_rx) = mpsc::channel::<Waiting>(8);

        let tunnel: TunnelListener = TunnelListener::new(
            Bytes::from_static(TOKEN.as_bytes()),
            certs,
            reverse_ports.into_iter().collect(),
            pending_rx,
        )
        .unwrap();
        let listener: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint: SocketAddr = listener.local_addr().unwrap();

        tokio::spawn(tunnel.run(listener));

        (pending_tx, endpoint)
    }

    fn spawn_target(
        endpoint: &SocketAddr,
        token: &str,
        fingerprint: Option<TunnelTlsFingerprint>,
        local_listen: Vec<SocketAddr>,
    ) {
        let connector: TunnelConnector = TunnelConnector::new(
            (*endpoint).into(),
            token.to_owned(),
            fingerprint,
            50,
            local_listen,
        );
        tokio::spawn(connector.run());
    }

    async fn connect_when_listening(addr: SocketAddr) -> TcpStream {
        for _ in 0..40 {
            if let Ok(Ok(stream)) =
                time::timeout(Duration::from_secs(1), TcpStream::connect(addr)).await
            {
                return stream;
            }
            time::sleep(Duration::from_millis(50)).await;
        }

        panic!("nothing listening on {addr}");
    }

    #[tokio::test]
    async fn tunnels_bidirectional_traffic() {
        let target: SocketAddr = spawn_echo().await;
        let certs: TunnelTlsCerts = TunnelTlsCerts::generate();

        let (pending_tx, endpoint): (mpsc::Sender<Waiting>, SocketAddr) =
            spawn_operator(&certs, []).await;
        spawn_target(&endpoint, TOKEN, Some(*certs.fingerprint()), vec![]);

        let pipe: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client: TcpStream = TcpStream::connect(pipe.local_addr().unwrap())
            .await
            .unwrap();
        let (mut server_end, _): (TcpStream, SocketAddr) = pipe.accept().await.unwrap();
        pending_tx
            .send(Waiting {
                stream: client,
                target: HostAndPort::new(target.ip().to_string(), target.port()),
            })
            .await
            .unwrap();

        // Round-trips flow all the way to the echo server and back.
        server_end.write_all(b"ping").await.unwrap();
        let mut buffer: [u8; 4] = [0; 4];
        server_end.read_exact(&mut buffer).await.unwrap();
        assert_eq!(&buffer, b"ping");

        server_end.write_all(b"pongpong").await.unwrap();
        let mut buffer: [u8; 8] = [0; 8];
        server_end.read_exact(&mut buffer).await.unwrap();
        assert_eq!(&buffer, b"pongpong");
    }

    #[tokio::test]
    async fn rejects_wrong_token() {
        let certs: TunnelTlsCerts = TunnelTlsCerts::generate();

        let (_pending_tx, endpoint): (mpsc::Sender<Waiting>, SocketAddr) =
            spawn_operator(&certs, []).await;

        // The worker streams are refused with 401: the connector reports the
        // permanent failure instead of retrying.
        let connector: TunnelConnector = TunnelConnector::new(
            endpoint.into(),
            "wrong".to_owned(),
            Some(*certs.fingerprint()),
            50,
            vec![],
        );

        let error: io::Error = connector.run().await.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn rejects_wrong_fingerprint() {
        let target: SocketAddr = spawn_echo().await;
        let certs: TunnelTlsCerts = TunnelTlsCerts::generate();

        let (pending_tx, endpoint): (mpsc::Sender<Waiting>, SocketAddr) =
            spawn_operator(&certs, []).await;

        // The TLS handshake fails and the session retries forever: the
        // tunnel must not carry any traffic.
        spawn_target(
            &endpoint,
            TOKEN,
            Some(TunnelTlsFingerprint::from_byte_array([0xAA; 32])),
            vec![],
        );

        let pipe: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client: TcpStream = TcpStream::connect(pipe.local_addr().unwrap())
            .await
            .unwrap();
        let (mut server_end, _): (TcpStream, SocketAddr) = pipe.accept().await.unwrap();
        pending_tx
            .send(Waiting {
                stream: client,
                target: HostAndPort::new(target.ip().to_string(), target.port()),
            })
            .await
            .unwrap();

        // Nothing ever comes back: the client is queued but never served.
        server_end.write_all(b"ping").await.unwrap();
        let mut buffer: [u8; 4] = [0; 4];
        let served =
            time::timeout(Duration::from_secs(1), server_end.read_exact(&mut buffer)).await;
        assert!(served.is_err());
    }

    #[tokio::test]
    async fn tunnels_reverse_streams() {
        let certs: TunnelTlsCerts = TunnelTlsCerts::generate();

        let dialed: SocketAddr = spawn_echo().await;

        let local_listen: Vec<SocketAddr> = vec![SocketAddr::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            dialed.port(),
        )];
        let callback_addr: SocketAddr = local_listen[0];

        let (_pending_tx, endpoint): (mpsc::Sender<Waiting>, SocketAddr) =
            spawn_operator(&certs, [dialed.port()]).await;
        spawn_target(&endpoint, TOKEN, Some(*certs.fingerprint()), local_listen);

        let mut callback: TcpStream = connect_when_listening(callback_addr).await;
        callback.write_all(b"hello").await.unwrap();

        let mut buffer: [u8; 5] = [0; 5];
        let echoed = time::timeout(Duration::from_secs(5), callback.read_exact(&mut buffer)).await;
        assert!(echoed.is_ok(), "the tunnel never echoed back");
        echoed.unwrap().unwrap();
        assert_eq!(&buffer, b"hello");
    }

    #[tokio::test]
    async fn rejects_undeclared_reverse_ports() {
        let certs: TunnelTlsCerts = TunnelTlsCerts::generate();

        let (_pending_tx, endpoint): (mpsc::Sender<Waiting>, SocketAddr) =
            spawn_operator(&certs, []).await;

        let local_listen: Vec<SocketAddr> =
            vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4444)];
        let callback_addr: SocketAddr = local_listen[0];
        spawn_target(&endpoint, TOKEN, Some(*certs.fingerprint()), local_listen);

        let mut callback: TcpStream = connect_when_listening(callback_addr).await;
        callback.write_all(b"hello").await.unwrap();

        let mut buffer: [u8; 5] = [0; 5];
        let served = time::timeout(Duration::from_secs(1), callback.read_exact(&mut buffer)).await;
        assert!(matches!(
            served,
            Ok(Err(error))
                if matches!(
                    error.kind(),
                    // A reset is expected when the socket is closed with
                    // unread data, an EOF otherwise.
                    ErrorKind::ConnectionReset | ErrorKind::UnexpectedEof
                )
        ));
    }
}
