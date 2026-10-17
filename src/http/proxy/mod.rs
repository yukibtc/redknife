use std::io;

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

mod handler;
mod queue;
mod state;

use self::handler::ProxyHandler;
pub(crate) use self::queue::PendingRequest;
use super::body::{HttpBody, HttpBodyError};
use super::compression::{UnpackError, unpack_response};
use crate::http::proxy::handler::{ProxyHandleDropReason, ProxyHandleOutcome};
use crate::http::request::HttpWireRequestError;

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
    pub fn new(listener: TcpListener, sender: mpsc::Sender<PendingRequest>) -> Self {
        Self {
            listener,
            handler: ProxyHandler::new(sender),
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
