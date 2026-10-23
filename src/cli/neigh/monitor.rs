use std::collections::HashMap;
use std::io;
use std::net::Ipv4Addr;

use crate::arp::ArpFrame;
use crate::net::{DataLinkSocket, Host, MacAddr};

pub(super) async fn run(iface: String) -> io::Result<()> {
    eprintln!("Using interface: {iface}");

    let host: Host = Host::read(&iface)?;

    eprintln!("Our MAC address: {}", host.mac());
    eprintln!("Our IPv4 address: {}", host.ip());

    let sock: DataLinkSocket = DataLinkSocket::new(&iface)?;
    let (_tx, mut rx) = sock.split();

    // ARP table
    let mut table: HashMap<Ipv4Addr, MacAddr> = HashMap::new();

    eprintln!("\nWaiting for ARP traffic...\n");

    loop {
        // Wait for a frame
        let frame: Vec<u8> = rx.next().await?;

        let Ok(arp) = ArpFrame::decode(&frame) else {
            continue;
        };

        // Discard our own packets
        if arp.eth_src == host.mac() {
            continue;
        }

        if arp.sender_mac().is_broadcast() || arp.sender_mac().is_unspecified() {
            continue;
        }

        match table.insert(arp.sender_ip(), arp.sender_mac()) {
            None => println!("{} -> {}", arp.sender_ip(), arp.sender_mac()),
            Some(old) if old != arp.sender_mac() => {
                eprintln!(
                    "{} changed MAC! {old} -> {}",
                    arp.sender_ip(),
                    arp.sender_mac()
                )
            }
            _ => {}
        }
    }
}
