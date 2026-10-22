use std::io;

mod host;
mod ioctl;
mod layer;
mod mac;
mod pcap;
mod sysfs;

pub use self::host::Host;
pub use self::ioctl::read_ipv4_netmask;
pub use self::layer::datalink::*;
pub use self::mac::{MacAddr, MacAddrParseError};
pub use self::pcap::PcapWriter;
pub use self::sysfs::read_ipv4_gateway_sysfs;

/// List the active network interfaces with an IPv4 address configured,
/// excluding loopback ones.
pub fn list_interfaces() -> io::Result<Vec<String>> {
    let mut ifaces: Vec<String> = Vec::new();

    for (name, ip) in ioctl::list_interfaces_ioctl()? {
        if !ip.is_loopback() && !ifaces.contains(&name) {
            ifaces.push(name);
        }
    }

    Ok(ifaces)
}
