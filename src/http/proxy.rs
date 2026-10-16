use std::io;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use http::header::CONNECTION;
use http::uri::InvalidUri;
use http::{HeaderMap, HeaderName, Method, Request, Response, StatusCode, Uri};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};
use tokio::task;

type ClientBuilder = hyper::client::conn::http1::Builder;
type ServerBuilder = hyper::server::conn::http1::Builder;
pub type ProxyBody = BoxBody<Bytes, hyper::Error>;

#[derive(Debug, Error)]
enum InnerError {
    #[error(transparent)]
    IO(#[from] io::Error),
    #[error(transparent)]
    Uri(#[from] InvalidUri),
    #[error(transparent)]
    Hyper(#[from] hyper::Error),
    #[error("missing host")]
    MissingHost,
    #[error("request dropped")]
    RequestDropped,
    #[error("expected an origin-form request target")]
    ExpectedOriginForm,
}

pub trait HttpProxyServerRequestHandler: Send + Sync {
    /// Receive an origin-form request without hop-by-hop headers.
    /// The original URI is separate metadata; its upstream destination stays fixed.
    /// Return the request to forward, or `None` to drop it.
    fn handle<'a>(
        &'a self,
        original_uri: Uri,
        req: Request<ProxyBody>,
    ) -> Pin<Box<dyn Future<Output = Option<Request<ProxyBody>>> + Send + 'a>>;
}

pub struct HttpProxyServer {
    listener: TcpListener,
    handler: Arc<dyn HttpProxyServerRequestHandler>,
}

impl HttpProxyServer {
    #[inline]
    pub fn new(listener: TcpListener, handler: Arc<dyn HttpProxyServerRequestHandler>) -> Self {
        Self { listener, handler }
    }

    pub async fn run(self) -> io::Result<()> {
        loop {
            let (stream, _) = self.listener.accept().await?;
            let io: TokioIo<TcpStream> = TokioIo::new(stream);
            let handler: Arc<dyn HttpProxyServerRequestHandler> = Arc::clone(&self.handler);

            task::spawn(async move {
                let service = service_fn(move |req| {
                    let handler: Arc<dyn HttpProxyServerRequestHandler> = Arc::clone(&handler);
                    async move { forward(req, handler.clone()).await }
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
    handler: Arc<dyn HttpProxyServerRequestHandler>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, InnerError> {
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
        .ok_or(InnerError::MissingHost)?;
    let port: u16 = original_uri.port_u16().unwrap_or(80);

    let mut req: Request<ProxyBody> = req.map(|body| body.boxed());

    prepare_origin_request(&mut req)?;

    let mut req = handler
        .handle(original_uri, req)
        .await
        .ok_or(InnerError::RequestDropped)?;

    if req.uri().scheme().is_some() || req.uri().authority().is_some() {
        return Err(InnerError::ExpectedOriginForm);
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

    let mut resp: Response<Incoming> = sender.send_request(req).await?;

    remove_hop_by_hop(resp.headers_mut());

    Ok(resp.map(|b| b.boxed()))
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

fn empty() -> ProxyBody {
    Empty::new().map_err(|never| match never {}).boxed()
}

fn full(text: &'static str) -> ProxyBody {
    Full::new(Bytes::from_static(text.as_bytes()))
        .map_err(|never| match never {})
        .boxed()
}

fn text_response(status: StatusCode, text: &'static str) -> Response<ProxyBody> {
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
    use tokio::net::TcpStream;
    use tokio::time;

    use super::*;

    struct FixedAction {
        drop: bool,
    }

    impl HttpProxyServerRequestHandler for FixedAction {
        fn handle<'a>(
            &'a self,
            _original_uri: Uri,
            req: Request<ProxyBody>,
        ) -> Pin<Box<dyn Future<Output = Option<Request<ProxyBody>>> + Send + 'a>> {
            Box::pin(async move { if self.drop { None } else { Some(req) } })
        }
    }

    struct EditTarget {
        target: Uri,
    }

    impl HttpProxyServerRequestHandler for EditTarget {
        fn handle<'a>(
            &'a self,
            _original_uri: Uri,
            mut req: Request<ProxyBody>,
        ) -> Pin<Box<dyn Future<Output = Option<Request<ProxyBody>>> + Send + 'a>> {
            Box::pin(async move {
                *req.uri_mut() = self.target.clone();
                Some(req)
            })
        }
    }

    async fn send_request(proxy_addr: SocketAddr, url: String) -> String {
        let req = format!(
            "POST {url} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nConnection: X-Proxy-Only\r\nX-Proxy-Only: remove\r\nProxy-Connection: keep-alive\r\nProxy-Authorization: Basic test\r\nX-Application: preserve\r\nContent-Length: 4\r\n\r\nping"
        );
        let mut stream = TcpStream::connect(proxy_addr).await.unwrap();
        stream.write_all(req.as_bytes()).await.unwrap();

        let mut response = String::new();

        time::timeout(Duration::from_secs(5), stream.read_to_string(&mut response))
            .await
            .unwrap()
            .unwrap();

        response
    }

    #[tokio::test]
    async fn forward_preserves_destination_and_sends_origin_form() {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();

        let upstream_task = task::spawn(async move {
            let (stream, _) = upstream.accept().await.unwrap();
            let service = service_fn(|req: Request<Incoming>| async move {
                assert_eq!(req.method(), Method::POST);
                assert_eq!(req.uri(), "/inspect?value=1");
                assert_eq!(req.headers()["host"], "localhost");
                assert_eq!(req.headers()["x-application"], "preserve");
                assert!(!req.headers().contains_key("proxy-connection"));
                assert!(!req.headers().contains_key("proxy-authorization"));
                assert!(!req.headers().contains_key("x-proxy-only"));

                let body = req.into_body().collect().await.unwrap().to_bytes();
                assert_eq!(body, Bytes::from_static(b"ping"));

                Ok::<_, hyper::Error>(Response::new(Full::new(body)))
            });
            ServerBuilder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
                .unwrap();
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = listener.local_addr().unwrap();

        let server = HttpProxyServer::new(listener, Arc::new(FixedAction { drop: false }));
        let proxy_task = task::spawn(server.run());

        let response = send_request(
            proxy_addr,
            format!("http://{upstream_addr}/inspect?value=1"),
        )
        .await;

        proxy_task.abort();

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response:?}");
        assert!(response.ends_with("\r\n\r\nping"), "{response:?}");

        upstream_task.await.unwrap();
    }

    #[tokio::test]
    async fn drop_closes_connection_without_contacting_upstream() {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = listener.local_addr().unwrap();

        let server = HttpProxyServer::new(listener, Arc::new(FixedAction { drop: true }));
        let proxy_task = task::spawn(server.run());

        let response = send_request(proxy_addr, format!("http://{upstream_addr}/")).await;
        proxy_task.abort();

        assert!(response.is_empty(), "{response:?}");

        let upstream = upstream.into_std().unwrap();

        assert_eq!(
            upstream.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[tokio::test]
    async fn absolute_and_authority_edits_never_contact_an_upstream() {
        let original = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let original_addr = original.local_addr().unwrap();
        let alternate = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let alternate_addr = alternate.local_addr().unwrap();

        for target in [
            format!("http://{original_addr}/edited"),
            format!("http://{alternate_addr}/edited"),
            alternate_addr.to_string(),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let proxy_addr = listener.local_addr().unwrap();
            let server = HttpProxyServer::new(
                listener,
                Arc::new(EditTarget {
                    target: target.parse().unwrap(),
                }),
            );
            let proxy_task = task::spawn(server.run());
            let response =
                send_request(proxy_addr, format!("http://{original_addr}/original")).await;
            proxy_task.abort();
            assert!(response.is_empty(), "{response:?}");
        }

        for upstream in [original, alternate] {
            assert_eq!(
                upstream.into_std().unwrap().accept().unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            );
        }
    }
}
