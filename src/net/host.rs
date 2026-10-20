use std::net::Ipv4Addr;

use crate::net::MacAddr;

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

    #[inline]
    pub fn ip(&self) -> Ipv4Addr {
        self.ip
    }

    #[inline]
    pub fn mac(&self) -> MacAddr {
        self.mac
    }
}
