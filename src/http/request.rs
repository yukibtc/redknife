use bytes::Bytes;
use http::header::{CONTENT_LENGTH, TRAILER, TRANSFER_ENCODING};
use http::request::Builder;
use http::{Request, Version};
use httparse::{EMPTY_HEADER, Request as HttpParseRequest, Status};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HttpWireRequestError {
    #[error(transparent)]
    Parse(#[from] httparse::Error),
    #[error(transparent)]
    Build(#[from] http::Error),
    #[error("Incomplete HTTP request headers (expected a blank line before the body)")]
    IncompleteHeaders,
    #[error("Only HTTP/1.0 and HTTP/1.1 are supported")]
    UnsupportedVersion,
}

/// Serialize the complete request, keeping arbitrary header and body bytes intact.
pub fn format_request(req: &Request<Bytes>) -> Result<Vec<u8>, HttpWireRequestError> {
    let version = match req.version() {
        Version::HTTP_10 => "HTTP/1.0",
        Version::HTTP_11 => "HTTP/1.1",
        _ => return Err(HttpWireRequestError::UnsupportedVersion),
    };

    let mut output: Vec<u8> = format!("{} {} {version}\r\n", req.method(), req.uri()).into_bytes();

    for (name, value) in req.headers() {
        output.extend_from_slice(name.as_str().as_bytes());
        output.extend_from_slice(b": ");
        output.extend_from_slice(value.as_bytes());
        output.extend_from_slice(b"\r\n");
    }

    output.extend_from_slice(b"\r\n");
    output.extend_from_slice(req.body());

    Ok(output)
}

/// The editor contains a decoded body, not HTTP transfer framing.
pub fn normalize_body_headers(req: &mut Request<Bytes>) {
    let has_body: bool = !req.body().is_empty()
        || req.headers().contains_key(CONTENT_LENGTH)
        || req.headers().contains_key(TRANSFER_ENCODING);

    let length: usize = req.body().len();

    req.headers_mut().remove(TRANSFER_ENCODING);
    req.headers_mut().remove(TRAILER);
    req.headers_mut().remove(CONTENT_LENGTH);

    if has_body {
        req.headers_mut().insert(CONTENT_LENGTH, length.into());
    }
}

/// Parse the request head with httparse; all remaining bytes are the edited body.
pub fn parse_request(raw: &[u8]) -> Result<Request<Bytes>, HttpWireRequestError> {
    let mut headers = [EMPTY_HEADER; 128];

    let mut head: HttpParseRequest = HttpParseRequest::new(&mut headers);

    let head_len: usize = match head.parse(raw)? {
        Status::Complete(length) => length,
        Status::Partial => return Err(HttpWireRequestError::IncompleteHeaders),
    };

    let version: Version = match head.version {
        Some(0) => Version::HTTP_10,
        Some(1) => Version::HTTP_11,
        _ => return Err(HttpWireRequestError::UnsupportedVersion),
    };

    let mut builder: Builder = Request::builder()
        .method(head.method.ok_or(HttpWireRequestError::IncompleteHeaders)?)
        .uri(head.path.ok_or(HttpWireRequestError::IncompleteHeaders)?)
        .version(version);

    for header in head.headers.iter() {
        builder = builder.header(header.name, header.value);
    }

    Ok(builder.body(Bytes::copy_from_slice(&raw[head_len..]))?)
}

#[cfg(test)]
mod tests {
    use http::header::HOST;

    use super::*;

    #[test]
    fn format_preserves_binary_headers_and_body() {
        let req = Request::builder()
            .method("POST")
            .uri("http://example.com/")
            .version(Version::HTTP_10)
            .header("x-binary", &b"\xff"[..])
            .body(Bytes::from_static(b"\x00\xff\x1b\r\n"))
            .unwrap();

        let raw = format_request(&req).unwrap();

        assert!(raw.starts_with(b"POST http://example.com/ HTTP/1.0\r\n"));
        assert!(raw.ends_with(b"x-binary: \xff\r\n\r\n\x00\xff\x1b\r\n"));
    }

    #[test]
    fn parse_keeps_repeated_headers_and_entire_edited_body() {
        let req = parse_request(
            b"PATCH http://example.com/new?q=1 HTTP/1.1\r\nx-test: one\r\nx-test: two\r\nContent-Length: 1\r\n\r\nfirst\nlast\n\n",
        ).unwrap();

        assert_eq!(req.method(), "PATCH");
        assert_eq!(req.uri(), "http://example.com/new?q=1");
        assert_eq!(req.headers().get_all("x-test").iter().count(), 2);
        assert!(!req.headers().contains_key(HOST));
        assert_eq!(req.body(), b"first\nlast\n\n".as_slice());
        assert_eq!(req.headers()[CONTENT_LENGTH], "1");
    }

    #[test]
    fn parse_preserves_transfer_headers_without_interpreting_the_body() {
        let req = parse_request(b"POST http://example.com/ HTTP/1.1\r\nTransfer-Encoding: chunked\r\nTrailer: x-checksum\r\n\r\nping").unwrap();

        assert_eq!(req.body(), b"ping".as_slice());
        assert!(!req.headers().contains_key(CONTENT_LENGTH));
        assert_eq!(req.headers()[TRANSFER_ENCODING], "chunked");
        assert_eq!(req.headers()[TRAILER], "x-checksum");
    }

    #[test]
    fn rejects_incomplete_or_unsupported_requests() {
        for raw in [
            &b"GET http://example.com/ HTTP/1.1\r\n"[..],
            &b"GET / HTTP/1.1\r\ninvalid header\r\n\r\n"[..],
            &b"GET http://example.com/ HTTP/2.0\r\n\r\n"[..],
        ] {
            assert!(parse_request(raw).is_err());
        }
    }

    #[test]
    fn empty_body_and_http_10_round_trip() {
        let req = parse_request(b"GET http://example.com/ HTTP/1.0\r\n\r\n").unwrap();

        assert_eq!(req.version(), Version::HTTP_10);
        assert!(req.body().is_empty());
        assert!(!req.headers().contains_key(CONTENT_LENGTH));

        let reparsed = parse_request(&format_request(&req).unwrap()).unwrap();

        assert_eq!(reparsed.uri(), req.uri());
        assert_eq!(reparsed.version(), req.version());
    }

    #[test]
    fn parse_accepts_relative_targets_without_adding_host() {
        let req = parse_request(b"GET /inspect HTTP/1.1\r\n\r\n").unwrap();

        assert_eq!(req.uri(), "/inspect");
        assert!(!req.headers().contains_key(HOST));
    }

    #[test]
    fn parse_does_not_limit_edited_body_size() {
        let mut raw = b"POST http://example.com/ HTTP/1.1\r\n\r\n".to_vec();

        let body_len = 10 * 1024 * 1024 + 1;
        raw.resize(raw.len() + body_len, b'x');

        let req = parse_request(&raw).unwrap();
        assert_eq!(req.body().len(), body_len);
        assert!(!req.headers().contains_key(CONTENT_LENGTH));
    }

    #[test]
    fn excess_headers_return_httparse_error() {
        let mut raw = "GET / HTTP/1.1\r\n".to_owned();

        raw.push_str(&"x-test: value\r\n".repeat(128));

        assert_eq!(
            parse_request(format!("{raw}\r\n").as_bytes())
                .unwrap()
                .headers()
                .get_all("x-test")
                .iter()
                .count(),
            128
        );

        raw.push_str("x-test: extra\r\n\r\n");

        assert!(matches!(
            parse_request(raw.as_bytes()),
            Err(HttpWireRequestError::Parse(httparse::Error::TooManyHeaders))
        ));
    }
}
