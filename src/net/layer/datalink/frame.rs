use std::ops::Deref;

use bytes::{Bytes, BytesMut};
use thiserror::Error;

use super::header::DataLinkHeader;
use crate::net::MacAddr;

#[derive(Debug, Error)]
pub enum DataLinkFrameError<P> {
    #[error("failed to decode payload: {0}")]
    Payload(P),
    #[error("frame too short")]
    TooShort,
}

pub trait DataLinkPayload: Sized {
    type Error: std::error::Error;

    fn ethertype(&self) -> u16;

    fn decode(ethertype: u16, buf: &[u8]) -> Result<Self, Self::Error>;

    fn encode(&self) -> Bytes;
}

pub struct DataLinkFrame<P> {
    pub eth_dst: MacAddr,
    pub eth_src: MacAddr,
    pub payload: P,
}

impl<P> DataLinkFrame<P>
where
    P: DataLinkPayload,
{
    #[inline]
    pub fn new(eth_dst: MacAddr, eth_src: MacAddr, payload: P) -> Self {
        Self {
            eth_dst,
            eth_src,
            payload,
        }
    }

    pub fn decode(buf: &[u8]) -> Result<Self, DataLinkFrameError<P::Error>> {
        if buf.len() < DataLinkHeader::LEN {
            return Err(DataLinkFrameError::TooShort);
        }

        let mut header: [u8; DataLinkHeader::LEN] = [0; DataLinkHeader::LEN];
        header.copy_from_slice(&buf[..DataLinkHeader::LEN]);
        let header: DataLinkHeader = DataLinkHeader::from_byte_array(header);

        let payload: &[u8] = &buf[DataLinkHeader::LEN..];
        let payload: P =
            P::decode(header.ethertype, payload).map_err(DataLinkFrameError::Payload)?;

        Ok(Self {
            eth_dst: header.eth_dst,
            eth_src: header.eth_src,
            payload,
        })
    }

    #[inline]
    pub fn header(&self) -> DataLinkHeader {
        DataLinkHeader {
            eth_dst: self.eth_dst,
            eth_src: self.eth_src,
            ethertype: self.payload.ethertype(),
        }
    }

    #[inline]
    pub fn payload(&self) -> &P {
        &self.payload
    }

    #[inline]
    pub fn encode(&self) -> Bytes {
        let header: [u8; 14] = self.header().to_bytes();
        let payload: Bytes = self.payload().encode();

        let mut bytes: BytesMut = BytesMut::with_capacity(header.len() + payload.len());
        bytes.extend(header);
        bytes.extend(payload);
        bytes.freeze()
    }
}

impl<P> Deref for DataLinkFrame<P> {
    type Target = P;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.payload
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DST: MacAddr = MacAddr::new([0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    const SRC: MacAddr = MacAddr::new([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);

    #[derive(Debug, Error)]
    #[error("Not a test payload")]
    struct NotTestPayload;

    #[derive(Debug, PartialEq, Eq)]
    struct TestPayload;

    impl DataLinkPayload for TestPayload {
        type Error = NotTestPayload;

        fn ethertype(&self) -> u16 {
            0x0001
        }

        fn decode(ethertype: u16, buf: &[u8]) -> Result<Self, Self::Error> {
            if ethertype != 0x0001 {
                return Err(NotTestPayload);
            }

            assert_eq!(buf.len(), 0);
            Ok(TestPayload)
        }

        fn encode(&self) -> Bytes {
            Bytes::new()
        }
    }

    fn frame() -> DataLinkFrame<TestPayload> {
        DataLinkFrame::new(DST, SRC, TestPayload)
    }

    #[test]
    fn roundtrip() {
        let frame = frame();
        let decoded: DataLinkFrame<TestPayload> = DataLinkFrame::decode(&frame.encode()).unwrap();

        assert_eq!(decoded.eth_dst, DST);
        assert_eq!(decoded.eth_src, SRC);
        assert_eq!(decoded.payload, TestPayload);
    }

    #[test]
    fn encode_header() {
        let bytes = frame().encode();

        assert_eq!(&bytes[0..6], DST.as_bytes());
        assert_eq!(&bytes[6..12], SRC.as_bytes());
        assert_eq!(u16::from_be_bytes([bytes[12], bytes[13]]), 0x0001);
    }

    #[test]
    fn header() {
        let header = frame().header();

        assert_eq!(header.eth_dst, DST);
        assert_eq!(header.eth_src, SRC);
        assert_eq!(header.ethertype, 0x0001);
    }

    #[test]
    fn decode_too_short() {
        let bytes = vec![0u8; 13];
        assert!(matches!(
            DataLinkFrame::<TestPayload>::decode(&bytes),
            Err(DataLinkFrameError::TooShort)
        ));
    }

    #[test]
    fn decode_payload_not_arp() {
        let mut bytes = frame().encode().to_vec();
        // Change ethertype to IPv4 while the payload is still ARP
        bytes[12] = 0x08;
        bytes[13] = 0x00;

        assert!(matches!(
            DataLinkFrame::<TestPayload>::decode(&bytes),
            Err(DataLinkFrameError::Payload(NotTestPayload))
        ));
    }
}
