use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time;

/// Outcome of a single TCP port probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PortState {
    /// The connection attempt succeeded.
    Open,
    /// The connection was refused.
    Closed,
    /// No response was received within the timeout.
    Filtered,
    /// Local resources were exhausted before the probe could reach the
    /// target, leaving the state undetermined.
    Unknown,
}

impl PortState {
    #[inline]
    pub(super) fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }

    #[inline]
    pub(super) fn is_closed(self) -> bool {
        matches!(self, Self::Closed)
    }

    #[inline]
    pub(super) fn is_unknown(self) -> bool {
        matches!(self, Self::Unknown)
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Filtered => "filtered",
            Self::Unknown => "unknown",
        }
    }
}

/// `ENFILE` (`WSAENFILE` on Windows): file table overflow.
#[cfg(unix)]
const ENFILE: i32 = 23;
/// `EMFILE` (`WSAEMFILE` on Windows): too many open files.
#[cfg(unix)]
const EMFILE: i32 = 24;
/// `EADDRNOTAVAIL` (`WSAEADDRNOTAVAIL` on Windows): no local source port left.
#[cfg(unix)]
const EADDRNOTAVAIL: i32 = 99;
/// `ENOMEM` (`WSAENOMEM` on Windows): out of kernel memory.
#[cfg(unix)]
const ENOMEM: i32 = 12;
/// `ENOBUFS` (`WSAENOBUFS` on Windows): out of kernel buffers.
#[cfg(unix)]
const ENOBUFS: i32 = 105;

/// `WSAENFILE`: file table overflow.
#[cfg(windows)]
const ENFILE: i32 = 10023;
/// `WSAEMFILE`: too many open files.
#[cfg(windows)]
const EMFILE: i32 = 10024;
/// `WSAEADDRNOTAVAIL`: no local source port left.
#[cfg(windows)]
const EADDRNOTAVAIL: i32 = 10049;
/// `WSAENOMEM`: out of kernel memory.
#[cfg(windows)]
const ENOMEM: i32 = 10012;
/// `WSAENOBUFS`: out of kernel buffers.
#[cfg(windows)]
const ENOBUFS: i32 = 10055;

/// Probe a TCP port by attempting a full connection.
#[inline]
pub(super) async fn probe(target: SocketAddr, timeout: Duration) -> PortState {
    match time::timeout(timeout, TcpStream::connect(target)).await {
        Ok(Ok(_)) => PortState::Open,
        Ok(Err(error)) => classify(&error),
        Err(_elapsed) => PortState::Filtered,
    }
}

/// Classify a connection error.
///
/// Only an explicit refusal from the remote host proves the port is closed:
/// every other error (unreachable network, exhausted local resources, ...)
/// leaves the state undetermined.
#[inline]
fn classify(error: &io::Error) -> PortState {
    match error.kind() {
        io::ErrorKind::ConnectionRefused => PortState::Closed,
        io::ErrorKind::OutOfMemory => PortState::Unknown,
        _ if is_resource_exhaustion(error) => PortState::Unknown,
        _ => PortState::Filtered,
    }
}

/// Whether the error reports exhausted local resources (file descriptors,
/// memory, or kernel buffers), in which case the probe never reached the
/// target and its outcome is meaningless.
#[inline]
fn is_resource_exhaustion(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(ENFILE | EMFILE | EADDRNOTAVAIL | ENOMEM | ENOBUFS)
    )
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn classifies_refusal_as_closed() {
        let error: io::Error = io::Error::from(io::ErrorKind::ConnectionRefused);
        assert_eq!(classify(&error), PortState::Closed);
    }

    #[test]
    fn classifies_timeout_and_other_errors_as_filtered() {
        let timeout: io::Error = io::Error::from(io::ErrorKind::TimedOut);
        assert_eq!(classify(&timeout), PortState::Filtered);

        // EHOSTUNREACH: the probe could not reach the target
        let network: io::Error = io::Error::from_raw_os_error(113);
        assert_eq!(classify(&network), PortState::Filtered);

        let other: io::Error = io::Error::other("something else");
        assert_eq!(classify(&other), PortState::Filtered);
    }

    #[test]
    fn classifies_resource_exhaustion_as_unknown() {
        for code in [ENFILE, EMFILE, EADDRNOTAVAIL, ENOMEM, ENOBUFS] {
            let error: io::Error = io::Error::from_raw_os_error(code);
            assert_eq!(classify(&error), PortState::Unknown, "code: {code}");
        }
    }
}
