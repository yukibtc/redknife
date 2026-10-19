use thiserror::Error;

use crate::net::{MacAddr, MacAddrParseError};

const HEADER_LEN: usize = 14;

#[derive(Debug, Error)]
pub enum DataLinkHeaderError {
    #[error(transparent)]
    MacAddr(#[from] MacAddrParseError),
    #[error("invalid header len ({0})")]
    InvalidHeaderLen(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DataLinkHeader {
    pub eth_dst: MacAddr,
    pub eth_src: MacAddr,
    /// The ETH type (0x0806 for ARP, 0x0800 IPv4, 0x86DD IPv6, etc.)
    pub ethertype: u16,
}

impl DataLinkHeader {
    pub const LEN: usize = HEADER_LEN;

    pub fn from_byte_array(bytes: [u8; HEADER_LEN]) -> Self {
        let eth_dst: [u8; 6] = [bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]];
        let eth_dst: MacAddr = MacAddr::new(eth_dst);

        let eth_src: [u8; 6] = [bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11]];
        let eth_src: MacAddr = MacAddr::new(eth_src);

        let ethertype: u16 = u16::from_be_bytes([bytes[12], bytes[13]]);

        Self {
            eth_dst,
            eth_src,
            ethertype,
        }
    }

    /// Extract the header from a byte slice.
    ///
    /// The byte slice can contain more data than the header, but only the first 14 bytes are used.
    pub fn extract(buf: &[u8]) -> Result<Self, DataLinkHeaderError> {
        if buf.len() < HEADER_LEN {
            return Err(DataLinkHeaderError::InvalidHeaderLen(buf.len()));
        }

        let mut bytes: [u8; HEADER_LEN] = [0; HEADER_LEN];
        bytes.copy_from_slice(&buf[0..HEADER_LEN]);

        Ok(Self::from_byte_array(bytes))
    }

    #[inline]
    pub fn to_bytes(self) -> [u8; HEADER_LEN] {
        let mut buf: [u8; HEADER_LEN] = [0; HEADER_LEN];
        buf[0..6].copy_from_slice(self.eth_dst.as_bytes());
        buf[6..12].copy_from_slice(self.eth_src.as_bytes());
        buf[12..14].copy_from_slice(&self.ethertype.to_be_bytes());
        buf
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
        let header = DataLinkHeader::from_byte_array(bytes);
        assert_eq!(
            header.eth_dst,
            MacAddr::new([0x00, 0x00, 0x00, 0x00, 0x00, 0x00])
        );
        assert_eq!(
            header.eth_src,
            MacAddr::new([0xff, 0xff, 0xff, 0xff, 0xff, 0xff])
        );
        assert_eq!(header.ethertype, 0x0806);
    }

    #[test]
    fn serialize_datalink_header() {
        let bytes = [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x08, 0x06,
        ];
        let header = DataLinkHeader::from_byte_array(bytes);
        assert_eq!(header.to_bytes(), bytes);
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
        assert_eq!(header.ethertype, 0x0806);
    }
}
