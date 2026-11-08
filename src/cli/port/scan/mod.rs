use std::collections::BTreeMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::num::{NonZeroU64, NonZeroUsize};
use std::ops::Range;
use std::time::Duration;

use futures_util::{StreamExt, stream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

mod probe;

use self::probe::PortState;
use super::resolve_host;
use crate::logger;

/// Default timeout for each port probe in milliseconds.
pub(super) const DEFAULT_TIMEOUT_MS: u64 = 1500;

/// Default number of ports probed concurrently.
const DEFAULT_CONCURRENCY: usize = 4500;

/// File descriptors kept out of the scanning budget for the process itself.
const PORTS: Range<u16> = 0..u16::MAX;

pub(super) async fn run(
    target: String,
    timeout: NonZeroU64,
    concurrency: Option<NonZeroUsize>,
) -> io::Result<()> {
    logger::init()?;

    let ip: IpAddr = resolve_host(&target).await?;
    let timeout: Duration = Duration::from_millis(timeout.get());

    let concurrency: usize = concurrency.map_or(DEFAULT_CONCURRENCY, NonZeroUsize::get);

    tracing::info!(
        "Scanning {ip} ({count} ports, concurrency: {concurrency}, timeout: {}ms)",
        timeout.as_millis(),
        count = PORTS.len(),
    );

    let (tx, mut rx) = mpsc::unbounded_channel();

    // Probe every port with the requested concurrency, streaming each
    // outcome back as soon as it is available.
    let scan: JoinHandle<()> = tokio::spawn(
        stream::iter(PORTS)
            .map(move |port| async move {
                (port, probe::probe(SocketAddr::new(ip, port), timeout).await)
            })
            .buffer_unordered(concurrency)
            .for_each(move |(port, state)| {
                let tx: mpsc::UnboundedSender<(u16, PortState)> = tx.clone();
                async move {
                    let _ = tx.send((port, state));
                }
            }),
    );

    let mut results: BTreeMap<u16, PortState> = BTreeMap::new();

    while let Some((port, state)) = rx.recv().await {
        results.insert(port, state);

        match state {
            PortState::Open => tracing::info!("{port}/tcp open"),
            PortState::Closed => tracing::debug!("{port}/tcp closed"),
            PortState::Filtered => tracing::debug!("{port}/tcp filtered"),
        }
    }

    scan.await.map_err(io::Error::other)?;

    let total: usize = results.len();
    let open: Vec<u16> = results
        .iter()
        .filter(|(_, state)| state.is_open())
        .map(|(port, _)| *port)
        .collect();
    let closed: usize = results.values().filter(|state| state.is_closed()).count();
    let filtered: usize = total - open.len() - closed;

    eprintln!(
        "\nScanned {total} ports on {ip}: {} open, {closed} closed, {filtered} filtered.",
        open.len(),
    );

    if open.is_empty() {
        eprintln!("No open ports found");
        return Ok(());
    }

    println!("{:<12}STATE", "PORT");
    for port in open {
        println!("{:<12}{}", format!("{port}/tcp"), PortState::Open.as_str(),);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::time::Duration;

    use tokio::net::TcpListener;

    use super::*;

    #[tokio::test]
    async fn probes_open_and_closed_ports() {
        // Open: a service is listening on the port
        let listener: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let open: SocketAddr = listener.local_addr().unwrap();
        assert_eq!(
            probe::probe(open, Duration::from_secs(1)).await,
            PortState::Open
        );

        // Closed: the port was bound, but nothing listens on it anymore
        let listener: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed: SocketAddr = listener.local_addr().unwrap();
        drop(listener);
        assert_eq!(
            probe::probe(closed, Duration::from_secs(1)).await,
            PortState::Closed,
        );
    }
}
