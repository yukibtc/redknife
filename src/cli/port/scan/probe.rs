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

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Filtered => "filtered",
        }
    }
}

/// Probe a TCP port by attempting a full connection.
#[inline]
pub(super) async fn probe(target: SocketAddr, timeout: Duration) -> PortState {
    match time::timeout(timeout, TcpStream::connect(target)).await {
        Ok(Ok(_)) => PortState::Open,
        Ok(Err(_)) => PortState::Closed,
        Err(_elapsed) => PortState::Filtered,
    }
}
