use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::time;

use crate::dns::{DnsClient, DnsRecord, DnsType, DomainName, system_nameserver};

pub(super) async fn run(
    domain: DomainName,
    r#type: Option<DnsType>,
    ip: Option<IpAddr>,
    port: u16,
    timeout: u64,
) -> io::Result<()> {
    let timeout: Duration = Duration::from_secs(timeout);

    let socket: UdpSocket = UdpSocket::bind("0.0.0.0:0").await?;

    let ip: IpAddr = match ip {
        Some(server) => server,
        None => system_nameserver()?,
    };
    let addr: SocketAddr = SocketAddr::new(ip, port);

    eprintln!("Server: {addr}\n");

    let client: DnsClient = DnsClient::new(socket);

    let r#type: DnsType = r#type.unwrap_or(DnsType::A);
    let records: Vec<DnsRecord> =
        time::timeout(timeout, client.query(addr, domain, r#type)).await??;

    for record in records {
        println!(
            "{} {} {} {}",
            record.name, record.ttl, record.r#type, record.rdata
        );
    }

    Ok(())
}
