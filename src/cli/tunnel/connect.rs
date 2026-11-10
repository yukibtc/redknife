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
    local_listen: Vec<SocketAddr>,
) -> io::Result<()> {
    logger::init()?;

    let connector: TunnelConnector = TunnelConnector::new(peer, auth, fingerprint, local_listen);

    tokio::select! {
        result = connector.run() => result,
        _ = signal::ctrl_c() => Ok(()),
    }
}
