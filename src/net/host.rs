use std::net::Ipv4Addr;

use tokio::io;

use super::ioctl::read_ipv4;
use super::mac::MacAddr;

/// Host IP and MAC address
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Host {
    ip: Ipv4Addr,
    mac: MacAddr,
}

impl Host {
    #[inline]
    pub fn new(ip: Ipv4Addr, mac: MacAddr) -> Self {
        Self { ip, mac }
    }

    /// Read IP and MAC address for the given interface
    pub fn read(iface: &str) -> io::Result<Self> {
        let our_mac: MacAddr = MacAddr::read(iface)?;
        let our_ip: Ipv4Addr = read_ipv4(iface)?;
        Ok(Self::new(our_ip, our_mac))
    }

    #[inline]
    pub fn ip(&self) -> Ipv4Addr {
        self.ip
    }

    #[inline]
    pub fn mac(&self) -> MacAddr {
        self.mac
    }
}
