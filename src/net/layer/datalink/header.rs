use etherparse::{EtherType, Ethernet2Header};
use thiserror::Error;

use crate::net::MacAddr;

#[derive(Debug, Error)]
pub enum DataLinkHeaderError {
    #[error("invalid header len ({0})")]
    InvalidHeaderLen(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataLinkHeader(Ethernet2Header);

impl DataLinkHeader {
    pub const LEN: usize = Ethernet2Header::LEN;

    pub fn new(eth_dst: MacAddr, eth_src: MacAddr, ethertype: u16) -> Self {
        Self(Ethernet2Header {
            source: eth_src.to_bytes(),
            destination: eth_dst.to_bytes(),
            ether_type: EtherType(ethertype),
        })
    }

    /// Extract the header from a byte slice.
    ///
    /// The byte slice can contain more data than the header, but only the first 14 bytes are used.
    pub fn extract(buf: &[u8]) -> Result<Self, DataLinkHeaderError> {
        let (header, _) = Ethernet2Header::from_slice(buf)
            .map_err(|_| DataLinkHeaderError::InvalidHeaderLen(buf.len()))?;

        Ok(Self(header))
    }

    #[inline]
    pub fn eth_dst(&self) -> MacAddr {
        MacAddr::new(self.0.destination)
    }

    #[inline]
    pub fn eth_src(&self) -> MacAddr {
        MacAddr::new(self.0.source)
    }

    #[inline]
    pub fn ethertype(&self) -> u16 {
        self.0.ether_type.0
    }

    #[inline]
    pub fn to_bytes(&self) -> [u8; Self::LEN] {
        self.0.to_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_datalink_header() {
        let bytes = [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x08, 0x06,
        ];
        let header = DataLinkHeader::extract(&bytes).unwrap();
        assert_eq!(
            header.eth_dst(),
            MacAddr::new([0x00, 0x00, 0x00, 0x00, 0x00, 0x00])
        );
        assert_eq!(
            header.eth_src(),
            MacAddr::new([0xff, 0xff, 0xff, 0xff, 0xff, 0xff])
        );
        assert_eq!(header.ethertype(), 0x0806);
    }

    #[test]
    fn serialize_datalink_header() {
        let header = DataLinkHeader::new(
            MacAddr::new([0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
            MacAddr::new([0xff, 0xff, 0xff, 0xff, 0xff, 0xff]),
            0x0806,
        );
        assert_eq!(
            header.to_bytes(),
            [
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x08, 0x06
            ]
        );
    }

    #[test]
    fn extract_header_from_long_frame() {
        let frame: &[u8; 26] = &[
            // HEADER
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x08, 0x06,
            // RANDOM
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        // The header parsing must not fail, also if we provided more than 14 bytes
        let header = DataLinkHeader::extract(frame).unwrap();
        assert_eq!(header.ethertype(), 0x0806);
    }

    #[test]
    fn extract_header_too_short() {
        let bytes = [0u8; 13];
        assert!(matches!(
            DataLinkHeader::extract(&bytes),
            Err(DataLinkHeaderError::InvalidHeaderLen(13))
        ));
    }
}
