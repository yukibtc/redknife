use std::io;
use std::net::Ipv4Addr;

use crate::arp::resolve_single_mac;
use crate::net::{DataLinkSocket, Host};

pub(super) async fn run(ip: Ipv4Addr, iface: String) -> io::Result<()> {
    eprintln!("Using interface: {iface}");

    let sender: Host = Host::read(&iface)?;

    let sock: DataLinkSocket = DataLinkSocket::new(&iface)?;
    let (tx, mut rx) = sock.split();

    match resolve_single_mac(&tx, &mut rx, sender, ip).await? {
        Some(mac) => println!("{mac}"),
        None => eprintln!("No response received"),
    }

    Ok(())
}
