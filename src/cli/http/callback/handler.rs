use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::{fmt, io};

use bytes::Bytes;
use chrono::{DateTime, Local};
use http::header::CONTENT_TYPE;
use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Uri};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;

use crate::http::server::HttpServerHandler;

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

pub(super) struct Service;

impl HttpServerHandler for Service {
    fn handle_request<'a>(
        &'a self,
        req: Request<Incoming>,
        client: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = Response<BoxBody<Bytes, io::Error>>> + Send + 'a>> {
        Box::pin(async move {
            let now: DateTime<Local> = Local::now();

            let (parts, body) = req.into_parts();

            let body = match Limited::new(body, MAX_BODY_SIZE).collect().await {
                Ok(body) => body.to_bytes(),
                Err(error) => {
                    tracing::error!("Failed to read request body: {error}");
                    return text_response(StatusCode::PAYLOAD_TOO_LARGE, "Payload too large\n");
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

            text_response(StatusCode::OK, "OK\n")
        })
    }
}

fn text_response(status: StatusCode, text: &'static str) -> Response<BoxBody<Bytes, io::Error>> {
    let body: Bytes = Bytes::from_static(text.as_bytes());
    let body = Full::new(body).map_err(|e| match e {}).boxed();

    let mut response = Response::new(body);

    *response.status_mut() = status;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );

    response
}
