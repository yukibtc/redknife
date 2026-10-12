use std::str::FromStr;

use http::Uri;
use http::uri::{Authority, InvalidUri};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HttpTargetError {
    #[error(transparent)]
    Uri(#[from] InvalidUri),
    #[error("expected HTTP or HTTPS")]
    UnexpectedScheme,
    #[error("must contain only an origin")]
    MustContainOnlyAnOrigin,
    #[error("missing authority")]
    MissingAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpTarget {
    host: String,
    port: u16,
    tls: bool,
}

impl HttpTarget {
    fn parse(url: &str) -> Result<Self, HttpTargetError> {
        let uri: Uri = url.parse()?;

        let tls: bool = match uri.scheme_str() {
            Some("http") => false,
            Some("https") => true,
            _ => {
                return Err(HttpTargetError::UnexpectedScheme);
            }
        };

        if uri.path_and_query().is_some_and(|pq| pq.as_str() != "/") {
            return Err(HttpTargetError::MustContainOnlyAnOrigin);
        }

        let authority: &Authority = uri.authority().ok_or(HttpTargetError::MissingAuthority)?;

        // Removes IPv6 brackets, if any.
        // Functions like `("[::1]", 8000).to_socket_addrs()` returns an error if brackets are present.
        let host: &str = authority.host().trim_matches(['[', ']']);

        Ok(Self {
            host: host.to_owned(),
            port: authority.port_u16().unwrap_or(if tls { 443 } else { 80 }),
            tls,
        })
    }

    #[inline]
    pub(super) fn host(&self) -> &str {
        &self.host
    }

    #[inline]
    pub(super) fn port(&self) -> u16 {
        self.port
    }

    #[inline]
    pub(super) fn is_tls(&self) -> bool {
        self.tls
    }
}

impl FromStr for HttpTarget {
    type Err = HttpTargetError;

    fn from_str(url: &str) -> Result<Self, Self::Err> {
        Self::parse(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ipv4_http_url_with_port() {
        let target: HttpTarget = HttpTarget::parse("http://127.0.0.1:8000").unwrap();
        assert_eq!(target.host(), "127.0.0.1");
        assert_eq!(target.port(), 8000);
        assert_eq!(target.is_tls(), false);
    }

    #[test]
    fn parse_ipv4_http_url_without_port() {
        let target: HttpTarget = HttpTarget::parse("http://127.0.0.1").unwrap();
        assert_eq!(target.host(), "127.0.0.1");
        assert_eq!(target.port(), 80);
        assert_eq!(target.is_tls(), false);
    }

    #[test]
    fn parse_ipv4_https_url_with_port() {
        let target: HttpTarget = HttpTarget::parse("https://127.0.0.1:8181").unwrap();
        assert_eq!(target.host(), "127.0.0.1");
        assert_eq!(target.port(), 8181);
        assert_eq!(target.is_tls(), true);
    }

    #[test]
    fn parse_ipv4_https_url_without_port() {
        let target: HttpTarget = HttpTarget::parse("https://127.0.0.1").unwrap();
        assert_eq!(target.host(), "127.0.0.1");
        assert_eq!(target.port(), 443);
        assert_eq!(target.is_tls(), true);
    }

    #[test]
    fn parse_ipv6_http_url_with_port() {
        let target: HttpTarget = HttpTarget::parse("http://[::1]:8000").unwrap();
        assert_eq!(target.host(), "::1");
        assert_eq!(target.port(), 8000);
        assert_eq!(target.is_tls(), false);
    }

    #[test]
    fn parse_ipv6_http_url_without_port() {
        let target: HttpTarget = HttpTarget::parse("http://[::1]").unwrap();
        assert_eq!(target.host(), "::1");
        assert_eq!(target.port(), 80);
        assert_eq!(target.is_tls(), false);
    }

    #[test]
    fn parse_ipv6_https_url_with_port() {
        let target: HttpTarget = HttpTarget::parse("https://[::1]:8181").unwrap();
        assert_eq!(target.host(), "::1");
        assert_eq!(target.port(), 8181);
        assert_eq!(target.is_tls(), true);
    }

    #[test]
    fn parse_ipv6_https_url_without_port() {
        let target: HttpTarget = HttpTarget::parse("https://[::1]").unwrap();
        assert_eq!(target.host(), "::1");
        assert_eq!(target.port(), 443);
        assert_eq!(target.is_tls(), true);
    }
}
