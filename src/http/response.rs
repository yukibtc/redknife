use std::io;

use bytes::Bytes;
use http::Response;
use http_body_util::BodyExt;
use http_body_util::combinators::BoxBody;
use hyper::body::Body;
use tokio::io::{AsyncWrite, AsyncWriteExt};

use super::compression::unpack_response;

/// Write metadata and an automatically unpacked body without collecting it.
pub async fn write_response<B, W>(
    response: Response<B>,
    is_head: bool,
    output: &mut W,
) -> io::Result<()>
where
    B: Body<Data = Bytes> + Send + Sync + Unpin + 'static,
    B::Error: std::error::Error + Send + Sync,
    W: AsyncWrite + Unpin,
{
    let mut response: Response<BoxBody<Bytes, io::Error>> = unpack_response(response, is_head);

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

#[cfg(test)]
mod tests {
    use std::io;

    use futures_util::stream;
    use http_body_util::{Full, StreamBody};
    use hyper::body::Frame;

    use super::*;

    #[tokio::test]
    async fn test_body_error_keeps_partial_output() {
        let frames: [Result<Frame<Bytes>, io::Error>; 2] = [
            Ok(Frame::data(Bytes::from_static(b"partial"))),
            Err(io::Error::other("body read failed")),
        ];
        let response: Response<StreamBody<_>> =
            Response::new(StreamBody::new(stream::iter(frames)));
        let mut output: Vec<u8> = Vec::new();

        let result = write_response(response, false, &mut output).await;

        assert!(result.is_err());
        assert!(output.ends_with(b"\r\n\r\npartial"));
    }

    #[tokio::test]
    async fn test_invalid_compression_keeps_response_headers() {
        let response: Response<Full<Bytes>> = Response::builder()
            .header("content-encoding", "gzip")
            .body(Full::new(Bytes::from_static(b"invalid gzip")))
            .unwrap();
        let mut output: Vec<u8> = Vec::new();

        let result = write_response(response, false, &mut output).await;

        assert!(result.is_err());
        assert_eq!(output, b"HTTP/1.1 200 OK\r\n\r\n");
    }
}
