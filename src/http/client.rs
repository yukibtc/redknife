use std::io;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::header::{HOST, InvalidHeaderValue};
use http::uri::InvalidUri;
use http::{Method, Request, Response};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use rustls_native_certs::CertificateResult;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::{task, time};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::crypto::CryptoProvider;
use tokio_rustls::rustls::pki_types::{InvalidDnsNameError, ServerName};
use tokio_rustls::rustls::{self, ClientConfig, RootCertStore};

use super::compression::{UnpackError, unpack_request};
use super::request::normalize_body_headers;
use super::response::write_response;
use super::target::HttpTarget;

#[derive(Debug, Error)]
pub enum HttpClientError {
    #[error("HTTP connection I/O error: {0}")]
    IO(#[from] io::Error),
    #[error("HTTP protocol error: {0}")]
    Hyper(#[from] hyper::Error),
    #[error("Invalid HTTP request URI: {0}")]
    Uri(#[from] InvalidUri),
    #[error("Invalid HTTP Host header: {0}")]
    Host(#[from] InvalidHeaderValue),
    #[error("Cannot unpack HTTP request: {0}")]
    Unpack(#[from] UnpackError),
    #[error("HTTP request timed out: {0}")]
    Timeout(#[from] time::error::Elapsed),
    #[error("TLS configuration error: {0}")]
    Tls(#[from] rustls::Error),
    #[error("Invalid TLS server name: {0}")]
    ServerName(#[from] InvalidDnsNameError),
    #[error("TLS certificate loading task failed: {0}")]
    CertificateTask(#[from] task::JoinError),
    #[error("No trusted TLS certificates: {errors:?}")]
    NoTrustedCertificates {
        errors: Vec<rustls_native_certs::Error>,
    },
}

pub async fn send_http<W>(
    mut request: Request<Bytes>,
    target: &HttpTarget,
    timeout: Duration,
    output: &mut W,
) -> Result<(), HttpClientError>
where
    W: AsyncWrite + Unpin,
{
    normalize_body_headers(&mut request);

    // If there is a scheme, remove it
    if request.uri().scheme().is_some() {
        *request.uri_mut() = request
            .uri()
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/")
            .parse()?;
    }

    // If miss the host, push it
    if !request.headers().contains_key(HOST) {
        request
            .headers_mut()
            .insert(HOST, target.host_and_port().parse()?);
    }

    time::timeout(timeout, async {
        // Unpack the request, if compressed.
        unpack_request(&mut request).await?;

        let stream: TcpStream = TcpStream::connect((target.host(), target.port())).await?;

        if target.is_tls() {
            let tls_stream: TlsStream<TcpStream> = connect_tls(stream, target.host()).await?;
            exchange(tls_stream, request, output).await
        } else {
            exchange(stream, request, output).await
        }
    })
    .await?
}

async fn connect_tls(
    stream: TcpStream,
    host: &str,
) -> Result<TlsStream<TcpStream>, HttpClientError> {
    let certificates: CertificateResult =
        task::spawn_blocking(rustls_native_certs::load_native_certs).await?;

    let mut roots: RootCertStore = RootCertStore::empty();

    roots.add_parsable_certificates(certificates.certs);

    if roots.is_empty() {
        return Err(HttpClientError::NoTrustedCertificates {
            errors: certificates.errors,
        });
    }

    let provider: Arc<CryptoProvider> = Arc::new(rustls::crypto::ring::default_provider());

    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();

    config.alpn_protocols = vec![b"http/1.1".to_vec()];

    let name: ServerName = ServerName::try_from(host.to_owned())?;

    Ok(TlsConnector::from(Arc::new(config))
        .connect(name, stream)
        .await?)
}

async fn exchange<S, W>(
    stream: S,
    request: Request<Bytes>,
    output: &mut W,
) -> Result<(), HttpClientError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    W: AsyncWrite + Unpin,
{
    let io: TokioIo<S> = TokioIo::new(stream);

    let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .handshake(io)
        .await?;

    let send = async {
        let is_head: bool = request.method() == Method::HEAD;
        let response: Response<Incoming> = sender.send_request(request.map(Full::new)).await?;
        drop(sender);
        write_response(response, is_head, output).await?;
        Ok::<(), HttpClientError>(())
    };

    let drive = async {
        connection.await?;
        Ok::<(), HttpClientError>(())
    };

    // Both futures stay in this scope, including when an error or timeout cancels them.
    tokio::try_join!(send, drive)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    use super::*;
    use crate::http::request::parse_request;

    #[tokio::test]
    async fn test_invalid_compression_fails_before_connecting() {
        let target: HttpTarget = "http://127.0.0.1:1".parse().unwrap();
        let request: Request<Bytes> =
            parse_request(b"POST / HTTP/1.1\r\nContent-Encoding: gzip\r\n\r\ninvalid").unwrap();
        let mut output: Vec<u8> = Vec::new();

        let result: Result<(), HttpClientError> =
            send_http(request, &target, Duration::from_secs(2), &mut output).await;

        assert!(matches!(result, Err(HttpClientError::Unpack(_))));
        assert!(output.is_empty());
    }

    #[tokio::test]
    async fn test_timeout_and_output_failure_keep_partial_output() {
        for fail_output in [false, true] {
            let listener: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();

            let target: HttpTarget = format!("http://{}", listener.local_addr().unwrap())
                .parse()
                .unwrap();

            let server: JoinHandle<()> = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();

                let mut request: Vec<u8> = Vec::new();

                while !request.ends_with(b"\r\n\r\n") {
                    request.push(stream.read_u8().await.unwrap());
                }

                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nab")
                    .await
                    .unwrap();

                let mut remaining: Vec<u8> = Vec::new();
                stream.read_to_end(&mut remaining).await.unwrap();
            });

            let request: Request<Bytes> = parse_request(b"GET / HTTP/1.1\r\n\r\n").unwrap();

            if fail_output {
                let (mut output, reader) = tokio::io::duplex(1);

                drop(reader);

                let error: HttpClientError =
                    send_http(request, &target, Duration::from_secs(2), &mut output)
                        .await
                        .unwrap_err();

                let HttpClientError::IO(error) = error else {
                    panic!("expected an output write error");
                };

                assert_eq!(error.kind(), ErrorKind::BrokenPipe);
            } else {
                let mut output: Vec<u8> = Vec::new();

                let error: HttpClientError =
                    send_http(request, &target, Duration::from_millis(200), &mut output)
                        .await
                        .unwrap_err();

                assert!(matches!(error, HttpClientError::Timeout(_)));
                assert!(output.ends_with(b"\r\n\r\nab"));
            }

            time::timeout(Duration::from_secs(2), server)
                .await
                .unwrap()
                .unwrap();
        }
    }
}
