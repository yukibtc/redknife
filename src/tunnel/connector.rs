use std::io::{self, ErrorKind};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use h2::client::{SendRequest, handshake};
use h2::{RecvStream, SendStream};
use http::{Method, Request, Response, StatusCode, Uri};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio::time;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::ClientConnection;
use tokio_rustls::rustls::pki_types::ServerName;

use super::{
    ALPN, DIAL_PATH, HDR_AUTHORIZATION, HDR_PORT, OPDIAL_PATH, TunnelTlsFingerprint, bridge, dial,
    extract_hdr_target, h2_to_io_err, resolve_target, tls,
};
use crate::host_and_port::HostAndPort;

const POOL_SIZE: usize = 50;

pub struct TunnelConnector {
    peer: HostAndPort,
    token: Arc<str>,
    fingerprint: Option<TunnelTlsFingerprint>,
    local_listen: Vec<SocketAddr>,
}

impl TunnelConnector {
    #[inline]
    pub fn new(
        peer: HostAndPort,
        token: String,
        fingerprint: Option<TunnelTlsFingerprint>,
        local_listen: Vec<SocketAddr>,
    ) -> Self {
        Self {
            peer,
            token: token.into(),
            fingerprint,
            local_listen,
        }
    }

    pub async fn run(self) -> io::Result<()> {
        // Network failures are retried with a growing backoff.
        let mut backoff: Duration = Duration::from_secs(1);

        loop {
            let result: io::Result<()> = session(
                &self.peer,
                &self.token,
                self.fingerprint,
                &self.local_listen,
            )
            .await;

            let Err(error) = result else { return Ok(()) };

            if !is_retryable(&error) {
                return Err(error);
            }

            tracing::warn!("Tunnel down ({error}); reconnecting in {backoff:?}");

            time::sleep(backoff).await;

            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    }
}

// Run one tunnel session until the connection drops or the task is canceled.
async fn session(
    peer: &HostAndPort,
    token: &Arc<str>,
    fingerprint: Option<TunnelTlsFingerprint>,
    local_listen: &[SocketAddr],
) -> io::Result<()> {
    let sender: SendRequest<Bytes> = connect(peer, fingerprint).await?;

    tracing::info!("Tunnel connected to {peer}");

    let mut tasks: JoinSet<io::Result<()>> = JoinSet::new();

    for _ in 0..POOL_SIZE {
        let sender: SendRequest<Bytes> = sender.clone();
        let token: Arc<str> = token.clone();
        tasks.spawn(worker(sender, token));
    }

    for addr in local_listen {
        let listener: TcpListener = TcpListener::bind(*addr).await?;

        tracing::info!("Reverse listener on {addr} (dialled by the operator on its own loopback)");

        let sender: SendRequest<Bytes> = sender.clone();
        let token: Arc<str> = token.clone();
        tasks.spawn(reverse_listener(listener, sender, token));
    }

    let result: io::Result<()> = match tasks.join_next().await {
        Some(joined) => match joined {
            Ok(result) => result,
            Err(error) => Err(io::Error::other(error)),
        },
        None => Ok(()),
    };

    tasks.abort_all();

    result
}

async fn connect(
    peer: &HostAndPort,
    fingerprint: Option<TunnelTlsFingerprint>,
) -> io::Result<SendRequest<Bytes>> {
    let addr: SocketAddr = resolve_target(peer).await?;
    let stream: TcpStream = dial(addr).await?;

    let tls_connector: TlsConnector = tls::connector(fingerprint)?;
    let server_name: ServerName<'static> = ServerName::try_from("redknife".to_owned())
        .map_err(|error| io::Error::new(ErrorKind::InvalidInput, error))?;
    let tls_stream: TlsStream<TcpStream> = tls_connector
        .connect(server_name, stream)
        .await
        .map_err(|error| io::Error::new(ErrorKind::ConnectionReset, error))?;

    let (_, client): (_, &ClientConnection) = tls_stream.get_ref();

    if client.alpn_protocol() != Some(ALPN) {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "unexpected TLS ALPN protocol",
        ));
    }

    let (sender, connection) = handshake(tls_stream).await.map_err(h2_to_io_err)?;

    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::debug!("HTTP/2 connection closed: {error}");
        }
    });

    Ok(sender)
}

async fn worker(mut sender: SendRequest<Bytes>, token: Arc<str>) -> io::Result<()> {
    let uri: Uri = uri(DIAL_PATH);

    loop {
        sender = sender.ready().await.map_err(h2_to_io_err)?;

        let request: Request<()> = Request::builder()
            .method(Method::POST)
            .uri(uri.clone())
            .header(HDR_AUTHORIZATION, format!("Bearer {token}"))
            .body(())
            .map_err(|error| io::Error::new(ErrorKind::InvalidInput, error))?;

        let (response, send): (_, SendStream<Bytes>) =
            sender.send_request(request, false).map_err(h2_to_io_err)?;

        let response: Response<RecvStream> = response.await.map_err(h2_to_io_err)?;

        if response.status() == StatusCode::NO_CONTENT {
            // The operator had no client for this worker, refill immediately.
            continue;
        }
        if response.status() != StatusCode::OK {
            // Refusals are permanent (i.e., a wrong token), tear the session down.
            let kind: ErrorKind = if response.status() == StatusCode::UNAUTHORIZED {
                ErrorKind::PermissionDenied
            } else {
                ErrorKind::InvalidData
            };

            return Err(io::Error::new(
                kind,
                format!("operator refused the worker stream: {}", response.status()),
            ));
        }

        let target: HostAndPort = match extract_hdr_target(response.headers()) {
            Ok(target) => target,
            Err(e) => {
                tracing::warn!("{e}");
                continue;
            }
        };

        let addr: SocketAddr = match resolve_target(&target).await {
            Ok(addr) => addr,
            Err(e) => {
                tracing::warn!("Cannot resolve {target}: {e}");
                continue;
            }
        };

        match dial(addr).await {
            Ok(tcp) => {
                // A stream-level failure (reset, cancellation, etc.) must not tear the session down.
                if let Err(e) = bridge(response.into_body(), send, tcp).await {
                    tracing::debug!("Stream to {target} closed: {e}");
                }
            }
            Err(e) => {
                // The target is not reachable from this side
                tracing::warn!("Cannot dial {target}: {e}");
            }
        }
    }
}

async fn reverse_listener(
    listener: TcpListener,
    mut sender: SendRequest<Bytes>,
    token: Arc<str>,
) -> io::Result<()> {
    let port: u16 = listener.local_addr()?.port();
    let uri: Uri = uri(OPDIAL_PATH);

    loop {
        let (client, peer): (TcpStream, SocketAddr) = listener.accept().await?;

        tracing::debug!("Reverse connection from {peer} -> {port} (operator side)");

        sender = sender.ready().await.map_err(h2_to_io_err)?;

        let request: Request<()> = Request::builder()
            .method(Method::POST)
            .uri(uri.clone())
            .header(HDR_PORT, port.to_string())
            .header(HDR_AUTHORIZATION, format!("Bearer {token}"))
            .body(())
            .map_err(|error| io::Error::new(ErrorKind::InvalidInput, error))?;

        let (response, send): (_, SendStream<Bytes>) =
            sender.send_request(request, false).map_err(h2_to_io_err)?;

        let response: Response<RecvStream> = response.await.map_err(h2_to_io_err)?;

        if response.status() != StatusCode::OK {
            tracing::warn!(
                "Operator could not dial connection for port {port}: {}",
                response.status()
            );
            continue;
        }

        tokio::spawn(async move {
            if let Err(error) = bridge(response.into_body(), send, client).await {
                tracing::debug!("Reverse stream for port {port} closed: {error}");
            }
        });
    }
}

// Build the request URI of a tunnel route
//
// The authority is a placeholder!
fn uri(path: &'static str) -> Uri {
    Uri::builder()
        .scheme("https")
        .authority("redknife")
        .path_and_query(path)
        .build()
        .expect("static tunnel URI must be valid")
}

fn is_retryable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::ConnectionRefused
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::BrokenPipe
            | ErrorKind::TimedOut
            | ErrorKind::UnexpectedEof
            | ErrorKind::Interrupted
            | ErrorKind::AddrNotAvailable
    )
}
