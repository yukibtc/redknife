use std::io;
use std::io::Error;
use std::net::SocketAddr;
use std::pin::Pin;

use bytes::Bytes;
use http::header::CONNECTION;
use http::uri::InvalidUri;
use http::{HeaderMap, HeaderName, Method, Request, Response, StatusCode, Uri};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full, Limited};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::task;

use super::compression::{UnpackError, unpack_request, unpack_response};
use super::constant::MAX_BODY_SIZE;
use super::request::{HttpWireRequestError, normalize_body_headers};
use super::server::HttpServerHandler;

type ClientBuilder = hyper::client::conn::http1::Builder;

#[derive(Debug, Error)]
enum HttpProxyError {
    #[error("HTTP proxy I/O error: {0}")]
    IO(#[from] io::Error),
    #[error("Invalid proxy request URI: {0}")]
    Uri(#[from] InvalidUri),
    #[error("HTTP proxy protocol error: {0}")]
    Hyper(#[from] hyper::Error),
    #[error("Cannot unpack intercepted request: {0}")]
    Unpack(#[from] UnpackError),
    #[error("HTTP wire request error: {0}")]
    HttpWire(#[from] HttpWireRequestError),
    #[error(transparent)]
    LimitedCollector(Box<dyn std::error::Error + Send + Sync>),
    #[error("missing host")]
    MissingHost,
    #[error("expected an origin-form request target")]
    ExpectedOriginForm,
    // #[error("request dropped")]
    // RequestDropped,
}

pub enum HttpProxyHandlerDecision {
    Forward(Box<Request<Bytes>>),
    Deny(StatusCode),
    //Drop,
}

pub trait HttpProxyHandler: Send + Sync {
    fn handle(
        &self,
        uri: Uri,
        req: Request<Bytes>,
    ) -> Pin<Box<dyn Future<Output = io::Result<HttpProxyHandlerDecision>> + Send + '_>>;
}

pub struct HttpProxyService<T> {
    handler: T,
}

impl<T> HttpProxyService<T>
where
    T: HttpProxyHandler,
{
    #[inline]
    pub fn new(handler: T) -> Self {
        Self { handler }
    }
}

impl<T> HttpServerHandler for HttpProxyService<T>
where
    T: HttpProxyHandler,
{
    fn handle_request<'a>(
        &'a self,
        req: Request<Incoming>,
        _client: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = Response<BoxBody<Bytes, Error>>> + Send + 'a>> {
        Box::pin(async move {
            forward(req, &self.handler).await.unwrap_or_else(|err| {
                tracing::error!("Error forwarding request: {}", err);
                text_response(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong.\n")
            })
        })
    }
}

// Inspired by https://github.com/hyperium/hyper/blob/ea5b49b7d4e18c1eaa7f42db0ff91d1dcca56a04/examples/http_proxy.rs
async fn forward<T>(
    req: Request<Incoming>,
    handler: &T,
) -> Result<Response<BoxBody<Bytes, io::Error>>, HttpProxyError>
where
    T: HttpProxyHandler,
{
    if req.method() == Method::CONNECT {
        return Ok(text_response(
            StatusCode::NOT_IMPLEMENTED,
            "HTTPS CONNECT is not supported\n",
        ));
    }

    if req.uri().scheme_str() != Some("http") || req.uri().host().is_none() {
        return Ok(text_response(
            StatusCode::BAD_REQUEST,
            "Expected an absolute HTTP URI\n",
        ));
    }

    let original_uri: Uri = req.uri().clone();
    let host: String = original_uri
        .host()
        .map(str::to_string)
        .ok_or(HttpProxyError::MissingHost)?;
    let port: u16 = original_uri.port_u16().unwrap_or(80);

    // Collect the body into Bytes, with a maximum size of MAX_BODY_SIZE
    let (parts, body) = req.into_parts();
    let bytes: Bytes = Limited::new(body, MAX_BODY_SIZE)
        .collect()
        .await
        .map_err(HttpProxyError::LimitedCollector)?
        .to_bytes();
    let mut req: Request<Bytes> = Request::from_parts(parts, bytes);

    prepare_origin_request(&mut req)?;

    // Unpack the compressed request
    unpack_request(&mut req).await?;

    // And normalize the body headers
    normalize_body_headers(&mut req);

    let mut req: Request<Bytes> = match handler.handle(original_uri, req).await? {
        HttpProxyHandlerDecision::Forward(req) => *req,
        HttpProxyHandlerDecision::Deny(code) => {
            return Ok(text_response(code, "Request blocked by redknife\n"));
        } // HttpProxyHandlerDecision::Drop => {
          //     return Err(HttpProxyError::RequestDropped);
          // }
    };

    if req.uri().scheme().is_some() || req.uri().authority().is_some() {
        return Err(HttpProxyError::ExpectedOriginForm);
    }

    remove_hop_by_hop(req.headers_mut());
    normalize_body_headers(&mut req);

    let req: Request<BoxBody<Bytes, io::Error>> =
        req.map(|body| Full::new(body).map_err(|never| match never {}).boxed());

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

fn empty() -> BoxBody<Bytes, io::Error> {
    Empty::new().map_err(|never| match never {}).boxed()
}

fn full(text: &'static str) -> BoxBody<Bytes, io::Error> {
    Full::new(Bytes::from_static(text.as_bytes()))
        .map_err(|never| match never {})
        .boxed()
}

fn text_response(status: StatusCode, text: &'static str) -> Response<BoxBody<Bytes, io::Error>> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(if text.is_empty() { empty() } else { full(text) })
        .expect("static response builder must be valid")
}
