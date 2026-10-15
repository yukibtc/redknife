use std::io;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::Request;
use http::header::HOST;
use http_body_util::Full;
use hyper_util::rt::TokioIo;
use rustls_native_certs::CertificateResult;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::{task, time};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::crypto::CryptoProvider;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{self, ClientConfig, RootCertStore};

use super::request::normalize_body_headers;
use super::response::write_response;
use super::target::HttpTarget;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub async fn send_http<W>(
    mut request: Request<Bytes>,
    target: &HttpTarget,
    timeout: Duration,
    output: &mut W,
) -> Result<()>
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

async fn connect_tls(stream: TcpStream, host: &str) -> Result<TlsStream<TcpStream>> {
    let certificates: CertificateResult =
        task::spawn_blocking(rustls_native_certs::load_native_certs).await?;

    let mut roots: RootCertStore = RootCertStore::empty();

    roots.add_parsable_certificates(certificates.certs);

    if roots.is_empty() {
        return Err(io::Error::other(format!(
            "No trusted TLS certificates: {:?}",
            certificates.errors
        ))
        .into());
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

async fn exchange<S, W>(stream: S, request: Request<Bytes>, output: &mut W) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    W: AsyncWrite + Unpin,
{
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;

    let send = async {
        let response = sender.send_request(request.map(Full::new)).await?;
        drop(sender);
        write_response(response, output).await
    };

    let drive = async {
        connection.await?;
        Ok(())
    };

    // Both futures stay in this scope, including when an error or timeout cancels them.
    tokio::try_join!(send, drive)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use crate::http::request::parse_request;

    #[tokio::test]
    async fn timeout_and_output_failure_close_the_connection_and_keep_partial_output() {
        for fail_output in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

            let target: HttpTarget = format!("http://{}", listener.local_addr().unwrap())
                .parse()
                .unwrap();

            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();

                let mut request = Vec::new();

                while !request.ends_with(b"\r\n\r\n") {
                    request.push(stream.read_u8().await.unwrap());
                }

                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nab")
                    .await
                    .unwrap();

                let mut remaining = Vec::new();
                stream.read_to_end(&mut remaining).await.unwrap();
            });

            let request = parse_request(b"GET / HTTP/1.1\r\n\r\n").unwrap();

            if fail_output {
                let (mut output, reader) = tokio::io::duplex(1);

                drop(reader);

                let error = send_http(request, &target, Duration::from_secs(2), &mut output)
                    .await
                    .unwrap_err();

                assert_eq!(
                    error.downcast_ref::<io::Error>().unwrap().kind(),
                    io::ErrorKind::BrokenPipe
                );
            } else {
                let mut output = Vec::new();

                let error = send_http(request, &target, Duration::from_millis(200), &mut output)
                    .await
                    .unwrap_err();

                assert!(error.is::<time::error::Elapsed>());
                assert!(output.ends_with(b"\r\n\r\nab"));
            }

            time::timeout(Duration::from_secs(2), server)
                .await
                .unwrap()
                .unwrap();
        }
    }
}
