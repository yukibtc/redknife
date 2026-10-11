use std::net::{IpAddr, SocketAddr};
use std::{fmt, io};

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Request};
use axum::{Router, body};
use chrono::{DateTime, Local};
use http::{HeaderMap, Method, StatusCode, Uri};
use tokio::net::TcpListener;
use tokio::signal;

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

async fn handle_request(
    ConnectInfo(client): ConnectInfo<SocketAddr>,
    request: Request,
) -> (StatusCode, &'static str) {
    let now: DateTime<Local> = Local::now();

    let (parts, body) = request.into_parts();

    let body: Bytes = match body::to_bytes(body, MAX_BODY_SIZE).await {
        Ok(body) => body,
        Err(error) => {
            eprintln!("Failed to read request body: {error}");
            return (StatusCode::PAYLOAD_TOO_LARGE, "Payload too large\n");
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

    println!("{output}");

    (StatusCode::OK, "OK\n")
}

pub async fn run(ip: IpAddr, port: u16) -> io::Result<()> {
    let addr: SocketAddr = SocketAddr::new(ip, port);

    let listener: TcpListener = TcpListener::bind(addr).await?;

    let app = Router::new().fallback(handle_request);

    println!("Listening at http://{}/", listener.local_addr()?);
    println!("Ctrl+C to stop.\n");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = signal::ctrl_c().await;
    })
    .await?;

    println!("\nListener stopped.");

    Ok(())
}
