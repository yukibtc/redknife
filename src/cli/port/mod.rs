use std::io;
use std::net::{IpAddr, SocketAddr};
use std::num::{NonZeroU64, NonZeroUsize};

use clap::Subcommand;
use tokio::net::lookup_host;

mod forward;
mod scan;

use self::scan::DEFAULT_TIMEOUT_MS;
use crate::host_and_port::HostAndPort;

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum PortCommand {
    /// Forward TCP connections from a listener to target
    #[command(arg_required_else_help = true)]
    Forward {
        /// Address to listen on (<ip>:<port>, e.g. 0.0.0.0:33060)
        listen: SocketAddr,
        /// Target to forward connections to (<host>:<port>, e.g. 172.16.0.10:3306).
        ///
        /// With --socks the hostname is passed to the proxy and resolved there,
        /// so names only reachable from the proxy's network work.
        target: HostAndPort,
        /// Relay connections through a SOCKS5 proxy (<ip>:<port>, no auth)
        #[arg(long)]
        socks: Option<SocketAddr>,
    },
    /// Scan TCP ports on a target
    #[command(arg_required_else_help = true)]
    Scan {
        /// Target to scan (IP or hostname)
        target: String,
        /// Timeout for each port probe in milliseconds
        #[arg(long, default_value_t = NonZeroU64::new(DEFAULT_TIMEOUT_MS).unwrap())]
        timeout: NonZeroU64,
        /// Maximum number of ports probed at the same time
        #[arg(long)]
        concurrency: Option<NonZeroUsize>,
    },
}

impl PortCommand {
    pub(in crate::cli) async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Forward {
                listen,
                target,
                socks,
            } => forward::run(listen, target, socks).await?,
            Self::Scan {
                target,
                timeout,
                concurrency,
            } => scan::run(target, timeout, concurrency).await?,
        }

        Ok(())
    }
}

/// Resolve a bare host (IP address or hostname) into an IP address.
#[inline]
async fn resolve_host(target: &str) -> io::Result<IpAddr> {
    lookup_host((target, 0))
        .await?
        .next()
        .map(|addr| addr.ip())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("failed to resolve target address: {target}"),
            )
        })
}
