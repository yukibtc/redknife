use std::io;
use std::sync::Arc;

use bytes::Bytes;
use http::header::CONNECTION;
use http::uri::InvalidUri;
use http::{HeaderMap, HeaderName, Method, Request, Response, StatusCode, Uri};
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task;

use crate::http::proxy::handler::{ProxyHandleDropReason, ProxyHandleOutcome};
use crate::http::request::HttpWireRequestError;

mod handler;
mod queue;
mod record;
mod state;

use self::handler::ProxyHandler;
pub(crate) use self::queue::{PendingRequest, RequestQueue};
pub(crate) use self::record::{RequestRecord, RequestStatus};
pub(crate) use self::state::ProxyState;
use super::body::{HttpBody, HttpBodyError};
use super::compression::{UnpackError, unpack_response};

type ClientBuilder = hyper::client::conn::http1::Builder;
type ServerBuilder = hyper::server::conn::http1::Builder;

#[derive(Debug, Error)]
enum HttpProxyError {
    #[error("HTTP proxy I/O error: {0}")]
    IO(#[from] io::Error),
    #[error("Invalid proxy request URI: {0}")]
    Uri(#[from] InvalidUri),
    #[error("HTTP proxy protocol error: {0}")]
    Hyper(#[from] hyper::Error),
    #[error("Cannot read intercepted request body: {0}")]
    Body(#[from] HttpBodyError),
    #[error("Cannot unpack intercepted request: {0}")]
    Unpack(#[from] UnpackError),
    #[error("HTTP wire request error: {0}")]
    HttpWire(#[from] HttpWireRequestError),
    #[error(transparent)]
    LimitedCollector(Box<dyn std::error::Error + Send + Sync>),
    #[error("missing host")]
    MissingHost,
    #[error("request dropped: {reason:?}")]
    RequestDropped { reason: ProxyHandleDropReason },
    #[error("expected an origin-form request target")]
    ExpectedOriginForm,
}

pub struct HttpProxyServer {
    listener: TcpListener,
    handler: ProxyHandler,
}

impl HttpProxyServer {
    #[inline]
    pub fn new(
        listener: TcpListener,
        sender: mpsc::Sender<PendingRequest>,
        state: Arc<ProxyState>,
    ) -> Self {
        Self {
            listener,
            handler: ProxyHandler::new(sender, state),
        }
    }

    pub async fn run(self) -> io::Result<()> {
        loop {
            let (stream, _) = self.listener.accept().await?;
            let io: TokioIo<TcpStream> = TokioIo::new(stream);
            let handler: ProxyHandler = self.handler.clone();

            task::spawn(async move {
                let service = service_fn(move |req| {
                    let handler: ProxyHandler = handler.clone();
                    async move { forward(req, handler).await }
                });

                let _ = ServerBuilder::new()
                    .preserve_header_case(true)
                    .title_case_headers(true)
                    .serve_connection(io, service)
                    .with_upgrades()
                    .await;
            });
        }
    }
}

// Inspired by https://github.com/hyperium/hyper/blob/ea5b49b7d4e18c1eaa7f42db0ff91d1dcca56a04/examples/http_proxy.rs
async fn forward(
    req: Request<Incoming>,
    handler: ProxyHandler,
) -> Result<Response<HttpBody>, HttpProxyError> {
    if req.method() == Method::CONNECT {
        return Ok(text_response(
            StatusCode::NOT_IMPLEMENTED,
            "HTTPS CONNECT is not supported",
        ));
    }

    if req.uri().scheme_str() != Some("http") || req.uri().host().is_none() {
        return Ok(text_response(
            StatusCode::BAD_REQUEST,
            "Expected an absolute HTTP URI",
        ));
    }

    let original_uri: Uri = req.uri().clone();
    let host: String = original_uri
        .host()
        .map(str::to_string)
        .ok_or(HttpProxyError::MissingHost)?;
    let port: u16 = original_uri.port_u16().unwrap_or(80);

    let mut req: Request<HttpBody> = req.map(|body| body.map_err(Into::into).boxed());

    prepare_origin_request(&mut req)?;

    let mut req: Request<HttpBody> = match handler.handle(original_uri, req).await? {
        ProxyHandleOutcome::Process(req) => *req,
        ProxyHandleOutcome::Drop(reason) => return Err(HttpProxyError::RequestDropped { reason }),
    };

    if req.uri().scheme().is_some() || req.uri().authority().is_some() {
        return Err(HttpProxyError::ExpectedOriginForm);
    }

    remove_hop_by_hop(req.headers_mut());

    let stream: TcpStream = TcpStream::connect((host.as_str(), port)).await?;
    let io: TokioIo<TcpStream> = TokioIo::new(stream);

    let (mut sender, conn) = ClientBuilder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .handshake(io)
        .await?;

    task::spawn(async move {
        let _ = conn.await;
    });

    let is_head: bool = req.method() == Method::HEAD;

    let mut resp: Response<Incoming> = sender.send_request(req).await?;

    remove_hop_by_hop(resp.headers_mut());

    Ok(unpack_response(resp, is_head))
}

fn prepare_origin_request<B>(req: &mut Request<B>) -> Result<(), InvalidUri> {
    if req.uri().scheme().is_some() {
        *req.uri_mut() = req
            .uri()
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/")
            .parse()?;
    }

    remove_hop_by_hop(req.headers_mut());
    Ok(())
}

fn remove_hop_by_hop(headers: &mut HeaderMap) {
    let connection_options: Vec<HeaderName> = headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect();

    for name in connection_options {
        headers.remove(name);
    }
    for name in [
        "connection",
        "proxy-connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ] {
        headers.remove(name);
    }
}

fn empty() -> HttpBody {
    Empty::new().map_err(|never| match never {}).boxed()
}

fn full(text: &'static str) -> HttpBody {
    Full::new(Bytes::from_static(text.as_bytes()))
        .map_err(|never| match never {})
        .boxed()
}

fn text_response(status: StatusCode, text: &'static str) -> Response<HttpBody> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(if text.is_empty() { empty() } else { full(text) })
        .expect("static response builder must be valid")
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::task::JoinHandle;
    use tokio::time::timeout;

    use super::*;
    use crate::http::request::format_request;

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    async fn upstream() -> (SocketAddr, mpsc::Receiver<Request<Bytes>>) {
        let listener: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address: SocketAddr = listener.local_addr().unwrap();

        let (sender, receiver) = mpsc::channel(1);

        task::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let service = service_fn(move |request: Request<Incoming>| {
                let sender: mpsc::Sender<Request<Bytes>> = sender.clone();
                async move {
                    let (parts, body) = request.into_parts();
                    let body: Bytes = body.collect().await.unwrap().to_bytes();
                    sender.send(Request::from_parts(parts, body)).await.unwrap();
                    Ok::<_, hyper::Error>(Response::new(Full::new(Bytes::from_static(b"ok"))))
                }
            });

            ServerBuilder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
                .unwrap();
        });

        (address, receiver)
    }

    async fn send_request(proxy_addr: SocketAddr, url: String) -> String {
        let request: String = format!(
            concat!(
                "POST {} HTTP/1.1\r\n",
                "Host: localhost\r\n",
                "Connection: close, X-Proxy-Only\r\n",
                "X-Proxy-Only: remove\r\n",
                "Proxy-Authorization: Basic test\r\n",
                "Content-Length: 4\r\n\r\nping",
            ),
            url,
        );

        timeout(TEST_TIMEOUT, async {
            let mut stream: TcpStream = TcpStream::connect(proxy_addr).await.unwrap();
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut response: String = String::new();
            stream.read_to_string(&mut response).await.unwrap();
            response
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn test_off_forwards_without_a_pending_decision() {
        let (address, mut received) = upstream().await;

        let (sender, receiver) = mpsc::channel(1);

        let state: Arc<ProxyState> = Arc::default();

        let listener: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr: SocketAddr = listener.local_addr().unwrap();

        let server: HttpProxyServer = HttpProxyServer::new(listener, sender, Arc::clone(&state));
        let proxy_task: JoinHandle<io::Result<()>> = task::spawn(server.run());

        let response: String = send_request(proxy_addr, format!("http://{address}/original")).await;

        proxy_task.abort();

        let request: Request<Bytes> = received.try_recv().unwrap();
        let history: Vec<Arc<RequestRecord>> = state.history_since(0).unwrap();

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.ends_with("\r\n\r\nok"));
        assert_eq!(request.uri(), "/original");
        assert_eq!(request.body(), b"ping".as_slice());
        assert!(!request.headers().contains_key("x-proxy-only"));
        assert!(!request.headers().contains_key("proxy-authorization"));
        assert!(!state.is_intercepting());
        assert!(receiver.is_empty());
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, RequestStatus::Forwarded);
        assert_eq!(history[0].raw, format_request(&request).unwrap());
    }

    #[tokio::test]
    async fn test_edited_request_uses_the_original_destination() {
        let (address, mut received) = upstream().await;

        let (sender, mut receiver) = mpsc::channel(1);

        let state: Arc<ProxyState> = Arc::default();

        state.toggle_intercepting();

        let listener: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr: SocketAddr = listener.local_addr().unwrap();

        let server: HttpProxyServer = HttpProxyServer::new(listener, sender, Arc::clone(&state));
        let proxy_task: JoinHandle<io::Result<()>> = task::spawn(server.run());

        let draft: &[u8] = concat!(
            "PATCH /edited HTTP/1.1\r\n",
            "Host: elsewhere.invalid:1234\r\n",
            "Content-Length: 4\r\n\r\n",
            "edited\0",
        )
        .as_bytes();

        let edit = async {
            let pending: PendingRequest = receiver.recv().await.unwrap();

            let id: u64 = pending.record().id;

            let mut queue: RequestQueue = RequestQueue::default();

            queue.push(pending);
            queue.set_draft(id, draft.to_vec());
            queue.resolve(0, true).unwrap();
        };

        let (response, ()) = tokio::join!(
            send_request(proxy_addr, format!("http://{address}/original")),
            edit
        );

        proxy_task.abort();

        let request: Request<Bytes> = received.try_recv().unwrap();
        let history: Vec<Arc<RequestRecord>> = state.history_since(0).unwrap();

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert_eq!(request.method(), Method::PATCH);
        assert_eq!(request.uri(), "/edited");
        assert_eq!(request.headers()["host"], "elsewhere.invalid:1234");
        assert_eq!(request.headers()["content-length"], "7");
        assert_eq!(request.body(), b"edited\0".as_slice());
        assert_eq!(history[0].status, RequestStatus::Forwarded);
        assert_eq!(history[0].raw, format_request(&request).unwrap());
    }

    #[tokio::test]
    async fn test_drop_records_the_original_without_contacting_upstream() {
        let upstream: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address: SocketAddr = upstream.local_addr().unwrap();

        let (sender, mut receiver) = mpsc::channel(1);

        let state: Arc<ProxyState> = Arc::default();

        state.toggle_intercepting();

        let listener: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr: SocketAddr = listener.local_addr().unwrap();

        let server: HttpProxyServer = HttpProxyServer::new(listener, sender, Arc::clone(&state));
        let proxy_task: JoinHandle<io::Result<()>> = task::spawn(server.run());

        let drop_request = async {
            let pending: PendingRequest = receiver.recv().await.unwrap();
            let original: Bytes = pending.record().raw.clone();
            let id: u64 = pending.record().id;

            let mut queue: RequestQueue = RequestQueue::default();

            queue.push(pending);
            queue.set_draft(id, b"invalid edited draft".to_vec());
            queue.resolve(0, false).unwrap();

            original
        };

        let (response, original) = tokio::join!(
            send_request(proxy_addr, format!("http://{address}/original")),
            drop_request
        );

        proxy_task.abort();

        let history: Vec<Arc<RequestRecord>> = state.history_since(0).unwrap();

        assert!(response.is_empty());
        assert_eq!(
            upstream.into_std().unwrap().accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(history[0].status, RequestStatus::Dropped);
        assert_eq!(history[0].raw, original);
    }
}
