use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::{fmt, io};

use bytes::Bytes;
use chrono::{DateTime, Local};
use http::header::CONTENT_TYPE;
use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Uri};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio::signal;
use tokio::task::JoinSet;

use crate::logger;

const MAX_BODY_SIZE: usize = 10 * 1024 * 1024;

struct Output {
    now: DateTime<Local>,
    client_ip: IpAddr,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
}

impl fmt::Display for Output {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "Received request @ {}",
            self.now.format("%Y-%m-%d %H:%M:%S")
        )?;
        writeln!(f, "{} -> {} {}", self.client_ip, self.method, self.uri)?;

        for (name, value) in &self.headers {
            writeln!(f, "{name}: {}", String::from_utf8_lossy(value.as_bytes()))?;
        }

        if !self.body.is_empty() {
            writeln!(f, "\n{}", String::from_utf8_lossy(&self.body))?;
        }

        Ok(())
    }
}

fn text_response(status: StatusCode, text: &'static str) -> Response<Full<Bytes>> {
    let mut response: Response<Full<Bytes>> =
        Response::new(Full::new(Bytes::from_static(text.as_bytes())));

    *response.status_mut() = status;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );

    response
}

async fn handle_request(
    client: SocketAddr,
    req: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let now: DateTime<Local> = Local::now();

    let (parts, body) = req.into_parts();

    let body = match Limited::new(body, MAX_BODY_SIZE).collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            tracing::error!("Failed to read request body: {error}");
            return Ok(text_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Payload too large\n",
            ));
        }
    };

    let output = Output {
        now,
        client_ip: client.ip(),
        method: parts.method,
        uri: parts.uri,
        headers: parts.headers,
        body,
    };

    // Write to stdout, so piping is possible. Tracing logs are written to stderr.
    println!("{output}");

    Ok(text_response(StatusCode::OK, "OK\n"))
}

pub async fn run(ip: IpAddr, port: u16) -> io::Result<()> {
    logger::init()?;

    let addr: SocketAddr = SocketAddr::new(ip, port);

    let listener: TcpListener = TcpListener::bind(addr).await?;

    let mut connections: JoinSet<()> = JoinSet::new();

    tracing::info!(
        "HTTP Callback listening on http://{}/",
        listener.local_addr()?
    );
    tracing::info!("Ctrl+C to stop.");

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, client) = result?;

                let io: TokioIo<TcpStream> = TokioIo::new(stream);

                connections.spawn(async move {
                    let service = service_fn(move |request| {
                        handle_request(client, request)
                    });

                    if let Err(error) = http1::Builder::new()
                        .serve_connection(io, service)
                        .await
                    {
                        tracing::error!("HTTP connection error: {error}");
                    }
                });
            }

            Some(result) = connections.join_next(), if !connections.is_empty() => {
                if let Err(error) = result {
                    tracing::error!("Connection task error: {error}");
                }
            }

            result = signal::ctrl_c() => {
                result?;
                break;
            }
        }
    }

    connections.shutdown().await;

    tracing::info!("Listener stopped.");

    Ok(())
}
