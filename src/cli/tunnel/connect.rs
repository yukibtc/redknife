use std::io;
use std::net::SocketAddr;

use tokio::signal;

use crate::host_and_port::HostAndPort;
use crate::logger;
use crate::tunnel::{TunnelConnector, TunnelTlsFingerprint};

pub(super) async fn run(
    peer: HostAndPort,
    auth: String,
    fingerprint: Option<TunnelTlsFingerprint>,
    dial_workers: usize,
    local_listen: Vec<SocketAddr>,
) -> io::Result<()> {
    logger::init()?;

    if dial_workers == 0 && local_listen.is_empty() {
        return Err(io::Error::other(
            "At least a dial worker or a local listen is required",
        ));
    }

    let connector: TunnelConnector =
        TunnelConnector::new(peer, auth, fingerprint, dial_workers, local_listen);

    tokio::select! {
        result = connector.run() => result,
        _ = signal::ctrl_c() => Ok(()),
    }
}
