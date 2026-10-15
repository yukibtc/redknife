use http::Response;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use tokio::io::{AsyncWrite, AsyncWriteExt};

/// Write response metadata and the body decoded by Hyper, without buffering the body.
pub async fn write_response<W>(
    mut response: Response<Incoming>,
    output: &mut W,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    W: AsyncWrite + Unpin,
{
    let mut head: Vec<u8> =
        format!("{:?} {}\r\n", response.version(), response.status()).into_bytes();

    for (name, value) in response.headers() {
        head.extend_from_slice(name.as_str().as_bytes());
        head.extend_from_slice(b": ");
        head.extend_from_slice(value.as_bytes());
        head.extend_from_slice(b"\r\n");
    }

    head.extend_from_slice(b"\r\n");
    output.write_all(&head).await?;

    while let Some(frame) = response.frame().await {
        if let Ok(data) = frame?.into_data() {
            output.write_all(&data).await?;
        }
    }

    Ok(())
}
