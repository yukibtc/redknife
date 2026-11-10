use std::collections::HashSet;
use std::io::{self, ErrorKind};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use h2::server::{SendResponse, handshake};
use h2::{RecvStream, SendStream};
use http::{Request, Response, StatusCode};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, mpsc};
use tokio::{task, time};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConnection;
use tokio_rustls::server::TlsStream;

use super::tls::TunnelTlsCerts;
use super::{
    ALPN, DIAL_PATH, DIAL_TIMEOUT, HDR_AUTHORIZATION, HDR_PORT, HDR_TARGET, OPDIAL_PATH, bridge,
    dial, h2_to_io_err,
};
use crate::host_and_port::HostAndPort;

const PAIR_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub struct Waiting {
    pub stream: TcpStream,
    pub target: HostAndPort,
}

type Pending = Arc<Mutex<mpsc::Receiver<Waiting>>>;

pub struct TunnelListener {
    acceptor: TlsAcceptor,
    token: Bytes,
    pending: Pending,
    reverse_ports_whitelist: Arc<HashSet<u16>>,
}

impl TunnelListener {
    #[inline]
    pub fn new<I>(
        token: Bytes,
        certs: &TunnelTlsCerts,
        reverse_ports_whitelist: I,
        pending: mpsc::Receiver<Waiting>,
    ) -> io::Result<Self>
    where
        I: IntoIterator<Item = u16>,
    {
        Ok(Self {
            acceptor: certs.acceptor()?,
            token,
            pending: Arc::new(Mutex::new(pending)),
            reverse_ports_whitelist: Arc::new(reverse_ports_whitelist.into_iter().collect()),
        })
    }

    pub async fn run(self, listener: TcpListener) -> io::Result<()> {
        loop {
            let (stream, peer): (TcpStream, SocketAddr) = listener.accept().await?;

            tracing::info!("Tunnel connection from {peer}");

            let acceptor: TlsAcceptor = self.acceptor.clone();
            let token: Bytes = self.token.clone();
            let pending: Pending = self.pending.clone();
            let reverse_ports_whitelist: Arc<HashSet<u16>> = self.reverse_ports_whitelist.clone();

            task::spawn(async move {
                if let Err(error) =
                    serve_connection(acceptor, token, pending, reverse_ports_whitelist, stream)
                        .await
                {
                    tracing::debug!("Tunnel connection {peer} closed: {error}");
                }
            });
        }
    }
}

// Run TLS and HTTP/2 over a tunnel connection, dispatching its streams.
async fn serve_connection(
    acceptor: TlsAcceptor,
    token: Bytes,
    pending: Pending,
    reverse_ports_whitelist: Arc<HashSet<u16>>,
    stream: TcpStream,
) -> io::Result<()> {
    let tls_stream: TlsStream<TcpStream> = acceptor
        .accept(stream)
        .await
        .map_err(|error| io::Error::new(ErrorKind::ConnectionReset, error))?;

    let (_, server): (_, &ServerConnection) = tls_stream.get_ref();

    if server.alpn_protocol() != Some(ALPN) {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "unexpected TLS ALPN protocol",
        ));
    }

    let mut connection = handshake(tls_stream).await.map_err(h2_to_io_err)?;

    while let Some(request) = connection.accept().await {
        let (request, respond): (Request<RecvStream>, SendResponse<Bytes>) =
            request.map_err(h2_to_io_err)?;

        let token: Bytes = token.clone();
        let pending: Pending = pending.clone();
        let reverse_ports_whitelist: Arc<HashSet<u16>> = reverse_ports_whitelist.clone();

        task::spawn(async move {
            if let Err(error) =
                handle_stream(request, respond, token, pending, reverse_ports_whitelist).await
            {
                tracing::debug!("Tunnel stream failed: {error}");
            }
        });
    }

    Ok(())
}

async fn handle_stream(
    request: Request<RecvStream>,
    mut respond: SendResponse<Bytes>,
    token: Bytes,
    pending: Pending,
    reverse_ports_whitelist: Arc<HashSet<u16>>,
) -> io::Result<()> {
    if !is_authorized(&request, &token) {
        return reply(&mut respond, StatusCode::UNAUTHORIZED).await;
    }

    match (request.method().as_str(), request.uri().path()) {
        ("POST", DIAL_PATH) => handle_dial(request, respond, pending).await,
        ("POST", OPDIAL_PATH) => handle_opdial(request, respond, reverse_ports_whitelist).await,
        _ => reply(&mut respond, StatusCode::NOT_FOUND).await,
    }
}

async fn handle_dial(
    request: Request<RecvStream>,
    mut respond: SendResponse<Bytes>,
    pending: Pending,
) -> io::Result<()> {
    let waiting: Option<Waiting> = time::timeout(PAIR_TIMEOUT, async {
        let mut pending = pending.lock().await;
        pending.recv().await
    })
    .await
    .unwrap_or(None);

    let Some(waiting) = waiting else {
        return reply(&mut respond, StatusCode::NO_CONTENT).await;
    };

    let response: Response<()> = Response::builder()
        .status(StatusCode::OK)
        .header(HDR_TARGET, waiting.target.to_string())
        .body(())
        .map_err(|error| io::Error::new(ErrorKind::InvalidInput, error))?;

    let send: SendStream<Bytes> = respond
        .send_response(response, false)
        .map_err(h2_to_io_err)?;

    bridge(request.into_body(), send, waiting.stream).await
}

async fn handle_opdial(
    request: Request<RecvStream>,
    mut respond: SendResponse<Bytes>,
    reverse_ports_whitelist: Arc<HashSet<u16>>,
) -> io::Result<()> {
    let port: u16 = request
        .headers()
        .get(HDR_PORT)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| {
            io::Error::new(ErrorKind::InvalidData, "missing or invalid target header")
        })?;

    if !reverse_ports_whitelist.contains(&port) {
        tracing::warn!("Unauthorized reverse stream for port {port}");
        return reply(&mut respond, StatusCode::FORBIDDEN).await;
    }

    let addr: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));

    let stream: TcpStream = match time::timeout(DIAL_TIMEOUT, dial(addr)).await {
        Ok(result) => result?,
        Err(_) => {
            return reply(&mut respond, StatusCode::BAD_GATEWAY).await;
        }
    };

    let response: Response<()> = Response::builder()
        .status(StatusCode::OK)
        .body(())
        .map_err(|error| io::Error::new(ErrorKind::InvalidInput, error))?;
    let send: SendStream<Bytes> = respond
        .send_response(response, false)
        .map_err(h2_to_io_err)?;

    bridge(request.into_body(), send, stream).await
}

#[inline]
fn is_authorized(request: &Request<RecvStream>, token: &Bytes) -> bool {
    request
        .headers()
        .get(HDR_AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|got| ct_eq(got.as_bytes(), token.as_ref()))
}

#[inline]
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// Send an empty response with the given status and end the stream.
async fn reply(respond: &mut SendResponse<Bytes>, status: StatusCode) -> io::Result<()> {
    let response: Response<()> = Response::builder()
        .status(status)
        .body(())
        .map_err(|error| io::Error::new(ErrorKind::InvalidInput, error))?;
    respond
        .send_response(response, true)
        .map(|_| ())
        .map_err(h2_to_io_err)
}
