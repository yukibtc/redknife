use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HostAndPortError {
    #[error("missing host in address")]
    MissingHost,
    #[error("missing port in address")]
    MissingPort,
    #[error("invalid port in address")]
    InvalidPort,
    #[error("invalid host in address")]
    InvalidHost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostAndPort {
    // Hostname or IP literal, without IPv6 brackets.
    host: String,
    port: u16,
}

impl HostAndPort {
    #[inline]
    pub fn new<H>(host: H, port: u16) -> Self
    where
        H: Into<String>,
    {
        Self {
            host: host.into(),
            port,
        }
    }

    #[inline]
    pub fn host(&self) -> &str {
        &self.host
    }

    #[inline]
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl fmt::Display for HostAndPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

impl From<SocketAddr> for HostAndPort {
    #[inline]
    fn from(addr: SocketAddr) -> Self {
        Self::new(addr.ip().to_string(), addr.port())
    }
}

impl FromStr for HostAndPort {
    type Err = HostAndPortError;

    fn from_str(addr: &str) -> Result<Self, Self::Err> {
        // Fast path: SocketAddr already handles "ipv4:port" and "[ipv6]:port"
        // (and strips the IPv6 brackets for us).
        if let Ok(socket_addr) = SocketAddr::from_str(addr) {
            return Ok(Self::new(socket_addr.ip().to_string(), socket_addr.port()));
        }

        parse_host_port(addr)
    }
}

/// Parse a "host:port" address without IP-literal fast path.
fn parse_host_port(addr: &str) -> Result<HostAndPort, HostAndPortError> {
    let (host, port) = match addr.find(']') {
        // "[host]:port" form
        Some(end) => {
            let host = addr.get(1..end).ok_or(HostAndPortError::InvalidHost)?;
            let rest = addr.get(end + 1..).unwrap_or("");
            let port = rest
                .strip_prefix(':')
                .ok_or(HostAndPortError::MissingPort)?;
            (host, port)
        }
        // "host:port" form
        None => {
            let (host, port) = addr.rsplit_once(':').ok_or(HostAndPortError::MissingPort)?;
            if host.contains(':') {
                // Bare IPv6 with port is ambiguous: URL conventions require brackets.
                return Err(HostAndPortError::InvalidHost);
            }
            (host, port)
        }
    };

    if host.is_empty() {
        return Err(HostAndPortError::MissingHost);
    }

    let port: u16 = port.parse().map_err(|_| HostAndPortError::InvalidPort)?;

    Ok(HostAndPort::new(host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ipv4_with_port() {
        let addr: HostAndPort = "127.0.0.1:8000".parse().unwrap();
        assert_eq!(addr.host(), "127.0.0.1");
        assert_eq!(addr.port(), 8000);
    }

    #[test]
    fn parse_ipv6_bracketed_with_port() {
        let addr: HostAndPort = "[::1]:8000".parse().unwrap();
        assert_eq!(addr.host(), "::1");
        assert_eq!(addr.port(), 8000);
    }

    #[test]
    fn parse_domain_with_port() {
        let addr: HostAndPort = "example.com:443".parse().unwrap();
        assert_eq!(addr.host(), "example.com");
        assert_eq!(addr.port(), 443);
    }

    #[test]
    fn display_ipv6_is_bracketed() {
        let addr: HostAndPort = "[::1]:8000".parse().unwrap();
        assert_eq!(addr.to_string(), "[::1]:8000");
    }

    #[test]
    fn display_ipv4_and_domain_are_plain() {
        assert_eq!(
            "127.0.0.1:8000".parse::<HostAndPort>().unwrap().to_string(),
            "127.0.0.1:8000"
        );
        assert_eq!(
            "example.com:443"
                .parse::<HostAndPort>()
                .unwrap()
                .to_string(),
            "example.com:443"
        );
    }

    #[test]
    fn parse_missing_port_fails() {
        assert!(matches!(
            "127.0.0.1".parse::<HostAndPort>(),
            Err(HostAndPortError::MissingPort)
        ));
        assert!(matches!(
            "example.com".parse::<HostAndPort>(),
            Err(HostAndPortError::MissingPort)
        ));
        assert!(matches!(
            "[::1]".parse::<HostAndPort>(),
            Err(HostAndPortError::MissingPort)
        ));
    }

    #[test]
    fn parse_missing_host_fails() {
        assert!(matches!(
            ":8080".parse::<HostAndPort>(),
            Err(HostAndPortError::MissingHost)
        ));
        assert!(matches!(
            "[]:8080".parse::<HostAndPort>(),
            Err(HostAndPortError::MissingHost)
        ));
    }

    #[test]
    fn parse_invalid_port_fails() {
        assert!(matches!(
            "127.0.0.1:http".parse::<HostAndPort>(),
            Err(HostAndPortError::InvalidPort)
        ));
        assert!(matches!(
            "127.0.0.1:99999".parse::<HostAndPort>(),
            Err(HostAndPortError::InvalidPort)
        ));
    }

    #[test]
    fn parse_unbracketed_ipv6_fails() {
        // Ambiguous: could be an IPv6 address without port.
        assert!(matches!(
            "::1:8080".parse::<HostAndPort>(),
            Err(HostAndPortError::InvalidHost)
        ));
    }
}
