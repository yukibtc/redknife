use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use http::{Request, Response};
use http_body_util::combinators::BoxBody;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;

type ServerBuilder = hyper::server::conn::http1::Builder;

#[allow(clippy::type_complexity)]
pub trait HttpServerHandler: Send + Sync {
    fn handle_request<'a>(
        &'a self,
        req: Request<Incoming>,
        client: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = Response<BoxBody<Bytes, io::Error>>> + Send + 'a>>;
}

pub struct HttpServer {
    listener: TcpListener,
    handler: Arc<dyn HttpServerHandler>,
}

impl HttpServer {
    #[inline]
    pub fn new(listener: TcpListener, handler: Arc<dyn HttpServerHandler>) -> Self {
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
                    let io: TokioIo<TcpStream> = TokioIo::new(stream);
                    let handler: Arc<dyn HttpServerHandler> = self.handler.clone();

                    connections.spawn(async move {
                        let service = service_fn(move |req| {
                            let handler: Arc<dyn HttpServerHandler> = handler.clone();
                            async move { Ok::<_, hyper::Error>(handler.handle_request(req, client).await) }
                        });

                        let _ = ServerBuilder::new()
                            .preserve_header_case(true)
                            .title_case_headers(true)
                            .serve_connection(io, service)
                            .with_upgrades()
                            .await;
                    });
                }

                Some(result) = connections.join_next(), if !connections.is_empty() => {
                    if let Err(error) = result {
                        tracing::error!("Connection task error: {error}");
                    }
                }
            }
        }

        connections.shutdown().await;

        Ok(())
    }
}
