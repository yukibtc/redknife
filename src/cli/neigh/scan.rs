use std::collections::{BTreeMap, HashSet};
use std::io;
use std::net::Ipv4Addr;
use std::num::{NonZeroU64, NonZeroUsize};
use std::time::Duration;

use ipnet::Ipv4Net;
use tokio::time;

use crate::arp::{collect_resolved_macs, probe_resolve_mac};
use crate::net::{DataLinkSocket, Host};

pub(super) async fn run(
    iface: String,
    cidr: Ipv4Net,
    timeout: NonZeroU64,
    cooldown: NonZeroU64,
    attempts: NonZeroUsize,
) -> io::Result<()> {
    eprintln!("Using interface: {iface}");
    eprintln!("Using CIDR: {cidr}");

    let timeout: Duration = Duration::from_secs(timeout.get());
    let cooldown: Duration = Duration::from_secs(cooldown.get());
    let attempts: usize = attempts.get();

    let sender: Host = Host::read(&iface)?;

    let sock: DataLinkSocket = DataLinkSocket::new(&iface)?;
    let (tx, mut rx) = sock.split();

    let mut target_ips: HashSet<Ipv4Addr> = cidr.hosts().filter(|ip| *ip != sender.ip()).collect();
    let mut output = BTreeMap::new();

    eprintln!();

    for round in 1..=attempts {
        for addr in target_ips.iter().copied() {
            probe_resolve_mac(&tx, sender, addr).await?;
        }

        eprintln!("Probed {} IPs.", target_ips.len());
        eprintln!("Waiting for responses...");

        collect_resolved_macs(&mut rx, sender, &mut target_ips, &mut output, timeout).await?;

        if target_ips.is_empty() {
            break;
        }

        eprintln!("Round {round}/{attempts} completed.");

        if round < attempts {
            time::sleep(cooldown).await;
        }
    }

    eprintln!();

    if output.is_empty() {
        eprintln!("No hosts found");
        return Ok(());
    }

    eprintln!("Discovered {} hosts in {cidr}.", output.len());

    println!("{:<16}MAC", "IP");
    for (ip, mac) in &output {
        println!("{ip:<16}{mac}");
    }

    Ok(())
}
