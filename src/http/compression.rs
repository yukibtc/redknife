//! HTTP content decoding shared by the repeater and interception proxy.

use std::io;
use std::io::Cursor;
use std::pin::Pin;

use async_compression::tokio::bufread::{
    BrotliDecoder, DeflateDecoder, GzipDecoder, ZlibDecoder, ZstdDecoder,
};
use bytes::{Bytes, BytesMut};
use futures_util::{TryStreamExt, stream};
use http::header::{CONTENT_ENCODING, CONTENT_LENGTH, TRAILER, TRANSFER_ENCODING};
use http::{HeaderMap, Request, Response, StatusCode};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::{Body, Frame};
use thiserror::Error;
use tokio::io::{AsyncBufRead, AsyncRead, AsyncReadExt, BufReader};
use tokio::task;
use tokio_util::io::{ReaderStream, StreamReader};

use super::body::{HttpBody, HttpBodyError};
use super::constant::MAX_BODY_SIZE;
use super::request::normalize_body_headers;

type Reader = Pin<Box<dyn AsyncBufRead + Send + Sync>>;

#[derive(Debug, Error)]
pub enum UnpackError {
    #[error(transparent)]
    IO(#[from] io::Error),
    #[error("Request body exceeds the unpack limit of 10 MiB")]
    TooLarge,
}

#[derive(Clone, Copy, Debug)]
enum Encoding {
    Gzip,
    Deflate,
    Brotli,
    Zstd,
}

/// Validate the entire encoding chain before transforming any bytes or headers.
/// Unknown or malformed encoding lists are left intact, including known layers.
fn encodings(headers: &HeaderMap) -> Option<Vec<Encoding>> {
    if !headers.contains_key(CONTENT_ENCODING) {
        return None;
    }

    let mut encodings: Vec<Encoding> = Vec::new();

    for value in headers.get_all(CONTENT_ENCODING) {
        for token in value.to_str().ok()?.split(',') {
            let encoding: Encoding = match token.trim().to_ascii_lowercase().as_str() {
                "gzip" | "x-gzip" => Encoding::Gzip,
                "deflate" => Encoding::Deflate,
                "br" => Encoding::Brotli,
                "zstd" => Encoding::Zstd,
                "identity" => continue,
                _ => return None,
            };

            encodings.push(encoding);
        }
    }

    Some(encodings)
}

async fn decoder(mut reader: Reader, encoding: Encoding) -> io::Result<Reader> {
    let decoded: Box<dyn AsyncRead + Send + Sync + Unpin> = match encoding {
        Encoding::Gzip => {
            let mut decoder: GzipDecoder<Reader> = GzipDecoder::new(reader);
            decoder.multiple_members(true);
            Box::new(decoder)
        }
        Encoding::Deflate => {
            // HTTP deflate uses zlib, but some clients/servers send raw DEFLATE.
            // Put the prefix back so neither decoder loses input, even across frames.
            let mut prefix: [u8; 2] = [0; 2];

            reader.read_exact(&mut prefix).await?;

            let zlib: bool = prefix[0] & 0x0f == 8
                && prefix[0] >> 4 <= 7
                && u16::from_be_bytes(prefix) % 31 == 0;

            let reader = BufReader::new(Cursor::new(prefix).chain(reader));

            if zlib {
                Box::new(ZlibDecoder::new(reader))
            } else {
                Box::new(DeflateDecoder::new(reader))
            }
        }
        Encoding::Brotli => Box::new(BrotliDecoder::new(reader)),
        Encoding::Zstd => {
            let mut decoder: ZstdDecoder<Reader> = ZstdDecoder::new(reader);
            decoder.multiple_members(true);
            Box::new(decoder)
        }
    };

    Ok(Box::pin(BufReader::new(decoded)))
}

/// Atomically unpack a compressed request.
///
/// Returns `false` if no compression is detected, `true` if the request has been unpacked.
///
/// An error leaves the original request available to the caller.
pub async fn unpack_request(request: &mut Request<Bytes>) -> Result<bool, UnpackError> {
    let Some(encodings) = encodings(request.headers()) else {
        return Ok(false);
    };

    let mut body: Bytes = request.body().clone();

    if body.len() > MAX_BODY_SIZE {
        return Err(UnpackError::TooLarge);
    }

    for encoding in encodings.into_iter().rev() {
        let reader: Reader = Box::pin(Cursor::new(body));

        let mut reader: Reader = decoder(reader, encoding).await?;

        let mut decoded: BytesMut = BytesMut::new();
        let mut buffer: [u8; 8192] = [0; 8192];

        loop {
            let count: usize = reader.read(&mut buffer).await?;

            if count == 0 {
                break;
            }

            if count > MAX_BODY_SIZE.saturating_sub(decoded.len()) {
                return Err(UnpackError::TooLarge);
            }

            decoded.extend_from_slice(&buffer[..count]);

            task::yield_now().await;
        }

        body = decoded.freeze();
    }

    // Replace the body with the decoded body and remove the Content-Encoding header.
    *request.body_mut() = body;
    request.headers_mut().remove(CONTENT_ENCODING);

    normalize_body_headers(request);

    Ok(true)
}

/// Unpack a compressed response.
///
/// Decode a response without collecting it. The caller supplies HEAD semantics,
/// since response metadata alone cannot identify a response to a HEAD request.
pub fn unpack_response<B>(response: Response<B>, is_head: bool) -> Response<HttpBody>
where
    B: Body<Data = Bytes> + Send + Sync + Unpin + 'static,
    B::Error: Into<HttpBodyError>,
{
    let status: StatusCode = response.status();

    let no_body: bool = is_head
        || status.is_informational()
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED;

    let encodings: Option<Vec<Encoding>> = if no_body {
        None
    } else {
        encodings(response.headers())
    };

    let Some(encodings) = encodings else {
        return response.map(|body| body.map_err(Into::into).boxed());
    };

    let (mut parts, body) = response.into_parts();

    let stream = body.into_data_stream().map_err(|error| {
        let error: HttpBodyError = error.into();
        io::Error::other(error)
    });

    // Initialize on the first body poll so even deflate's prefix detection does
    // not delay response headers or lose them on cancellation/network failure.
    let stream = stream::once(async move {
        let mut reader: Reader = Box::pin(StreamReader::new(stream));

        for encoding in encodings.into_iter().rev() {
            reader = decoder(reader, encoding).await?;
        }

        Ok::<_, io::Error>(ReaderStream::new(reader))
    })
    .try_flatten();

    for header in [CONTENT_ENCODING, CONTENT_LENGTH, TRANSFER_ENCODING, TRAILER] {
        parts.headers.remove(header);
    }

    let stream = stream.map_ok(Frame::data).map_err(HttpBodyError::IO);

    Response::from_parts(parts, StreamBody::new(stream).boxed())
}

#[cfg(test)]
mod tests {
    use async_compression::tokio::bufread::{
        BrotliEncoder, DeflateEncoder, GzipEncoder, ZlibEncoder, ZstdEncoder,
    };
    use futures_util::stream;
    use http::StatusCode;
    use http_body_util::{Empty, Full};
    use tokio::io::AsyncRead;

    use super::*;

    async fn encode(encoding: &str, bytes: &[u8]) -> Bytes {
        let mut reader: Box<dyn AsyncRead + Unpin + Send + '_> = match encoding {
            "gzip" => Box::new(GzipEncoder::new(bytes)),
            "deflate" => Box::new(ZlibEncoder::new(bytes)),
            "raw-deflate" => Box::new(DeflateEncoder::new(bytes)),
            "br" => Box::new(BrotliEncoder::new(bytes)),
            "zstd" => Box::new(ZstdEncoder::new(bytes)),
            _ => panic!("unknown test encoding"),
        };
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.unwrap();
        Bytes::from(bytes)
    }

    fn request(encoding: &str, body: Bytes) -> Request<Bytes> {
        Request::builder()
            .method("POST")
            .uri("/")
            .header(CONTENT_ENCODING, encoding)
            .header(CONTENT_LENGTH, body.len())
            .header("content-type", "application/octet-stream")
            .body(body)
            .unwrap()
    }

    #[tokio::test]
    async fn request_formats_are_unpacked_losslessly_and_headers_are_normalized() {
        let plain = b"hello\x00\xff\r\nworld\n";
        for encoding in ["gzip", "deflate", "raw-deflate", "br", "zstd"] {
            let body = encode(encoding, plain).await;
            let header = if encoding == "raw-deflate" {
                "deflate"
            } else {
                encoding
            };
            let mut request = request(header, body);
            request
                .headers_mut()
                .insert(TRANSFER_ENCODING, "chunked".parse().unwrap());
            request
                .headers_mut()
                .insert(TRAILER, "x-checksum".parse().unwrap());

            assert!(unpack_request(&mut request).await.unwrap(), "{encoding}");
            assert_eq!(request.body(), plain.as_slice());
            assert_eq!(request.headers()[CONTENT_LENGTH], plain.len().to_string());
            assert_eq!(
                request.headers()["content-type"],
                "application/octet-stream"
            );
            for header in [CONTENT_ENCODING, TRANSFER_ENCODING, TRAILER] {
                assert!(!request.headers().contains_key(header));
            }
            assert!(!unpack_request(&mut request).await.unwrap());
        }
    }

    #[tokio::test]
    async fn repeated_case_insensitive_encoding_lists_are_decoded_in_reverse_order() {
        let plain = b"nested message";
        let gzip = encode("gzip", plain).await;
        let brotli = encode("br", &gzip).await;
        let mut request = request(" GZip, identity ", brotli);
        request
            .headers_mut()
            .append(CONTENT_ENCODING, " BR ".parse().unwrap());

        unpack_request(&mut request).await.unwrap();
        assert_eq!(request.body(), plain.as_slice());

        let mut identity = request.clone();
        identity
            .headers_mut()
            .insert(CONTENT_ENCODING, "identity".parse().unwrap());
        unpack_request(&mut identity).await.unwrap();
        assert_eq!(identity.body(), plain.as_slice());
        assert!(!identity.headers().contains_key(CONTENT_ENCODING));
    }

    #[tokio::test]
    async fn concatenated_gzip_members_and_zstd_frames_are_all_decoded() {
        for encoding in ["gzip", "zstd"] {
            let first = encode(encoding, b"first").await;
            let second = encode(encoding, b"second").await;
            let body = Bytes::from([first.as_ref(), second.as_ref()].concat());
            let mut request = request(encoding, body);
            unpack_request(&mut request).await.unwrap();
            assert_eq!(request.body(), b"firstsecond".as_slice());
        }
    }

    #[tokio::test]
    async fn unknown_or_malformed_lists_preserve_the_entire_message() {
        for encoding in ["custom", "custom, gzip", "gzip, custom", "gzip,", ""] {
            let body = encode("gzip", b"original").await;
            let mut request = request(encoding, body.clone());
            let headers = request.headers().clone();
            assert!(!unpack_request(&mut request).await.unwrap());
            assert_eq!(request.body(), &body);
            assert_eq!(request.headers(), &headers);

            let response = Response::builder()
                .header(CONTENT_ENCODING, encoding)
                .header(CONTENT_LENGTH, body.len())
                .body(Full::new(body.clone()))
                .unwrap();
            let headers = response.headers().clone();
            let response = unpack_response(response, false);
            assert_eq!(response.headers(), &headers);
            assert_eq!(response.collect().await.unwrap().to_bytes(), body);
        }
    }

    #[tokio::test]
    async fn corrupt_and_truncated_requests_fail_without_changing_original_bytes() {
        for encoding in ["gzip", "deflate", "br", "zstd"] {
            let encoded = encode(encoding, b"test content").await;
            for body in [
                Bytes::from_static(b"invalid"),
                encoded.slice(..encoded.len() - 1),
            ] {
                let mut request = request(encoding, body.clone());
                let headers = request.headers().clone();
                assert!(
                    matches!(unpack_request(&mut request).await, Err(UnpackError::IO(_))),
                    "{encoding}"
                );
                assert_eq!(request.body(), &body);
                assert_eq!(request.headers(), &headers);
            }
        }
    }

    #[tokio::test]
    async fn request_limit_applies_to_wire_intermediate_and_final_bodies() {
        let at_limit = vec![b'x'; MAX_BODY_SIZE];
        let mut boundary = request("gzip", encode("gzip", &at_limit).await);
        unpack_request(&mut boundary).await.unwrap();
        assert_eq!(boundary.body().len(), MAX_BODY_SIZE);

        let over_limit = vec![b'x'; MAX_BODY_SIZE + 1];
        let compressed = encode("gzip", &over_limit).await;
        for (encoding, body) in [
            ("gzip", Bytes::from(over_limit)),
            ("gzip", compressed.clone()),
            // The outer layer expands past the limit before the inner layer is read.
            ("gzip, gzip", compressed),
        ] {
            let mut request = request(encoding, body.clone());
            assert!(matches!(
                unpack_request(&mut request).await,
                Err(UnpackError::TooLarge)
            ));
            assert_eq!(request.body(), &body);
            assert_eq!(request.headers()[CONTENT_ENCODING], encoding);
        }
    }

    #[tokio::test]
    async fn response_formats_decode_across_single_byte_frames() {
        let plain = b"<html>readable response</html>\n";
        for encoding in ["gzip", "deflate", "raw-deflate", "br", "zstd"] {
            let encoded = encode(encoding, plain).await;
            let frames: Vec<Result<_, io::Error>> = encoded
                .iter()
                .map(|byte| Ok(Frame::data(Bytes::copy_from_slice(&[*byte]))))
                .collect();
            let header = if encoding == "raw-deflate" {
                "deflate"
            } else {
                encoding
            };
            let response = Response::builder()
                .header(CONTENT_ENCODING, header)
                .header(CONTENT_LENGTH, encoded.len())
                .header(TRANSFER_ENCODING, "chunked")
                .header(TRAILER, "x-checksum")
                .header("content-type", "text/html")
                .body(StreamBody::new(stream::iter(frames)))
                .unwrap();
            let response = unpack_response(response, false);
            for header in [CONTENT_ENCODING, CONTENT_LENGTH, TRANSFER_ENCODING, TRAILER] {
                assert!(!response.headers().contains_key(header));
            }
            assert_eq!(response.headers()["content-type"], "text/html");
            assert_eq!(
                response.collect().await.unwrap().to_bytes(),
                plain.as_slice(),
                "{encoding}"
            );
        }
    }

    #[tokio::test]
    async fn head_and_bodyless_statuses_keep_representation_headers() {
        for (is_head, status) in [
            (true, StatusCode::OK),
            (false, StatusCode::NO_CONTENT),
            (false, StatusCode::NOT_MODIFIED),
            (false, StatusCode::EARLY_HINTS),
        ] {
            let response = Response::builder()
                .status(status)
                .header(CONTENT_ENCODING, "gzip")
                .header(CONTENT_LENGTH, 123)
                .body(Empty::<Bytes>::new())
                .unwrap();
            let response = unpack_response(response, is_head);
            assert_eq!(response.headers()[CONTENT_ENCODING], "gzip");
            assert_eq!(response.headers()[CONTENT_LENGTH], "123");
            assert!(response.collect().await.unwrap().to_bytes().is_empty());
        }
    }

    #[tokio::test]
    async fn corrupt_response_is_a_body_error() {
        for encoding in ["gzip", "deflate", "br", "zstd"] {
            let response = Response::builder()
                .header(CONTENT_ENCODING, encoding)
                .body(Full::new(Bytes::from_static(b"invalid")))
                .unwrap();
            let response = unpack_response(response, false);
            assert!(response.collect().await.is_err(), "{encoding}");
        }
    }
}
