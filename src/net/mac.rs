use std::io::ErrorKind;
use std::str::FromStr;
use std::{fmt, io};

use thiserror::Error;

use super::ioctl::read_mac_addr_ioctl;
#[cfg(target_os = "linux")]
use super::sysfs::read_mac_addr_sysfs;

const LEN: usize = 6;

#[derive(Debug, Error)]
#[error("Invalid MAC address")]
pub struct MacAddrParseError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MacAddr([u8; LEN]);

impl MacAddr {
    /// Unspecified MAC address
    pub const UNSPECIFIED: Self = Self([0u8; LEN]);

    /// Broadcast MAC address
    pub const BROADCAST: Self = Self([0xff; LEN]);

    #[inline]
    pub const fn new(addr: [u8; LEN]) -> Self {
        Self(addr)
    }

    pub const fn from_slice(bytes: &[u8]) -> Result<Self, MacAddrParseError> {
        if bytes.len() != LEN {
            return Err(MacAddrParseError);
        }

        let mut addr: [u8; LEN] = [0; LEN];
        addr.copy_from_slice(bytes);

        Ok(Self(addr))
    }

    /// Read MAC address for a given interface
    #[inline]
    pub fn read(iface: &str) -> io::Result<Self> {
        // Primary: ioctl(SIOCGIFHWADDR)
        if let Ok(mac) = read_mac_addr_ioctl(iface) {
            if !mac.is_unspecified() {
                return Ok(mac);
            }
        }

        // Fallback: sysfs (works even where ioctl misbehaves)
        #[cfg(target_os = "linux")]
        if let Ok(mac) = read_mac_addr_sysfs(iface) {
            if !mac.is_unspecified() {
                return Ok(mac);
            }
        }

        Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("interface '{iface}' has no MAC address"),
        ))
    }

    #[inline]
    pub const fn as_bytes(&self) -> &[u8; LEN] {
        &self.0
    }

    #[inline]
    pub fn is_unspecified(&self) -> bool {
        self == &Self::UNSPECIFIED
    }

    #[inline]
    pub fn is_broadcast(&self) -> bool {
        self == &Self::BROADCAST
    }
}

impl fmt::Display for MacAddr {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

impl FromStr for MacAddr {
    type Err = MacAddrParseError;

    fn from_str(addr: &str) -> Result<Self, Self::Err> {
        // Accept both ':' and '-' as separators, but require a consistent one
        let sep: char = addr
            .chars()
            .find(|c| *c == ':' || *c == '-')
            .ok_or(MacAddrParseError)?;

        let parts: Vec<&str> = addr.split(sep).collect();

        if parts.len() != LEN {
            return Err(MacAddrParseError);
        }

        let mut mac: [u8; LEN] = [0u8; LEN];

        for (i, p) in parts.iter().enumerate() {
            if p.len() != 2 || !p.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(MacAddrParseError);
            }

            mac[i] = u8::from_str_radix(p, 16).map_err(|_| MacAddrParseError)?;
        }

        Ok(Self(mac))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_str() {
        let mac = MacAddr::BROADCAST;
        assert_eq!(mac.to_string(), "ff:ff:ff:ff:ff:ff");
    }

    #[test]
    fn parse_str() {
        let mac = MacAddr::from_str("ff:ff:ff:ff:ff:ff").unwrap();
        assert_eq!(mac, MacAddr::BROADCAST);

        let mac = MacAddr::from_str("00:00:00:00:00:00").unwrap();
        assert_eq!(mac, MacAddr([0x00; LEN]));
    }

    #[test]
    fn parse_str_with_dashes() {
        let mac: MacAddr = "aa-bb-cc-dd-ee-ff".parse().unwrap();
        assert_eq!(mac.to_string(), "aa:bb:cc:dd:ee:ff");
    }

    #[test]
    fn parse_str_invalid() {
        assert!(MacAddr::from_str("aa:bb:cc:dd:ee").is_err()); // too short
        assert!(MacAddr::from_str("aa:bb:cc:dd:ee:ff:00").is_err()); // too long
        assert!(MacAddr::from_str("aa:bb:cc-dd:ee:ff").is_err()); // mixed separators
        assert!(MacAddr::from_str("aa:bb:cc:dd:ee:gg").is_err()); // not hex
        assert!(MacAddr::from_str("a:b:c:d:e:f").is_err()); // wrong part length
        assert!(MacAddr::from_str("aa::cc:dd:ee:ff").is_err()); // empty part
        assert!(MacAddr::from_str("aabbccddeeff").is_err()); // no separator
    }
}
