use std::net::Ipv4Addr;

use bytes::Bytes;
use etherparse::err::arp::ArpEthIpv4FromError;
use etherparse::{ArpEthIpv4Packet, ArpOperation, EtherType};
use thiserror::Error;

use crate::net::{DataLinkPayload, MacAddr};

const OPCODE_REQUEST: u16 = 1;
const OPCODE_REPLY: u16 = 2;

#[derive(Debug, Error)]
pub enum ArpPacketError {
    #[error("frame too short")]
    TooShort(usize),
    #[error("not ARP ({0:#06x})")]
    NotArp(u16),
    #[error(transparent)]
    EthIpv4(#[from] ArpEthIpv4FromError),
    #[error("unknown opcode ({0})")]
    UnknownOpcode(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Hash)]
pub enum ArpOpCode {
    Request,
    Reply,
}

impl ArpOpCode {
    fn from_u16(opcode: u16) -> Result<Self, ArpPacketError> {
        match opcode {
            OPCODE_REQUEST => Ok(Self::Request),
            OPCODE_REPLY => Ok(Self::Reply),
            _ => Err(ArpPacketError::UnknownOpcode(opcode)),
        }
    }

    fn as_u16(self) -> u16 {
        match self {
            Self::Request => OPCODE_REQUEST,
            Self::Reply => OPCODE_REPLY,
        }
    }

    #[inline]
    pub fn is_reply(self) -> bool {
        matches!(self, Self::Reply)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArpPacket(ArpEthIpv4Packet);

impl ArpPacket {
    #[cfg(test)]
    const LEN: usize = ArpEthIpv4Packet::LEN;

    pub fn new(
        opcode: ArpOpCode,
        sender_mac: MacAddr,
        sender_ip: Ipv4Addr,
        target_mac: MacAddr,
        target_ip: Ipv4Addr,
    ) -> Self {
        Self(ArpEthIpv4Packet {
            operation: ArpOperation::from(opcode.as_u16()),
            sender_mac: sender_mac.to_bytes(),
            sender_ipv4: sender_ip.octets(),
            target_mac: target_mac.to_bytes(),
            target_ipv4: target_ip.octets(),
        })
    }

    pub fn opcode(&self) -> ArpOpCode {
        // Opcodes are validated on construction (`new` and `decode`)
        match self.0.operation.0 {
            OPCODE_REPLY => ArpOpCode::Reply,
            _ => ArpOpCode::Request,
        }
    }

    #[inline]
    pub fn sender_mac(&self) -> MacAddr {
        MacAddr::new(self.0.sender_mac)
    }

    #[inline]
    pub fn sender_ip(&self) -> Ipv4Addr {
        self.0.sender_ipv4_addr()
    }

    #[inline]
    pub fn target_mac(&self) -> MacAddr {
        MacAddr::new(self.0.target_mac)
    }

    #[inline]
    pub fn target_ip(&self) -> Ipv4Addr {
        self.0.target_ipv4_addr()
    }

    /// Returns `true` if it's a gratuitous ARP.
    ///
    /// In a gratuitous ARP the sender IP and target IP are the same. The target
    /// MAC is the sender MAC in a gratuitous reply, and unspecified (all zeros)
    /// in a gratuitous request.
    #[inline]
    pub fn is_gratuitous(&self) -> bool {
        self.sender_ip() == self.target_ip()
            && (self.sender_mac() == self.target_mac() || self.target_mac().is_unspecified())
    }
}

impl DataLinkPayload for ArpPacket {
    type Error = ArpPacketError;

    fn ethertype(&self) -> u16 {
        EtherType::ARP.0
    }

    fn decode(ethertype: u16, buf: &[u8]) -> Result<Self, Self::Error> {
        if ethertype != EtherType::ARP.0 {
            return Err(ArpPacketError::NotArp(ethertype));
        }

        let packet = etherparse::ArpPacket::from_slice(buf)
            .map_err(|_| ArpPacketError::TooShort(buf.len()))?;
        let packet: ArpEthIpv4Packet = packet.try_eth_ipv4()?;

        // Reject unknown opcodes so callers can match exhaustively on `ArpOpCode`
        ArpOpCode::from_u16(packet.operation.0)?;

        Ok(Self(packet))
    }

    fn encode(&self) -> Bytes {
        Bytes::copy_from_slice(&self.0.to_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{DataLinkFrame, DataLinkPayload};

    const ETHERTYPE_ARP: u16 = EtherType::ARP.0;
    const ETHERTYPE_IPV4: u16 = EtherType::IPV4.0;

    fn mac(b: [u8; 6]) -> MacAddr {
        MacAddr::new(b)
    }

    fn sample(opcode: ArpOpCode, target_mac: MacAddr) -> ArpPacket {
        ArpPacket::new(
            opcode,
            mac([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]),
            Ipv4Addr::new(192, 168, 1, 10),
            target_mac,
            Ipv4Addr::new(192, 168, 1, 1),
        )
    }

    /// Golden bytes of a valid ARP request (28 bytes).
    fn known_request_bytes() -> Vec<u8> {
        [
            0x00, 0x01, // htype: Ethernet
            0x08, 0x00, // ptype: IPv4
            0x06, 0x04, // hlen, plen
            0x00, 0x01, // opcode: request
            0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, // sender MAC
            192, 168, 1, 10, // sender IP
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // target MAC (unspecified)
            192, 168, 1, 1, // target IP
        ]
        .to_vec()
    }

    #[test]
    fn encode_known_bytes() {
        let packet = sample(ArpOpCode::Request, MacAddr::UNSPECIFIED);
        assert_eq!(packet.encode().to_vec(), known_request_bytes());
        assert_eq!(packet.encode().len(), ArpPacket::LEN);
    }

    #[test]
    fn decode_known_bytes() {
        let packet = ArpPacket::decode(ETHERTYPE_ARP, &known_request_bytes()).unwrap();
        assert_eq!(packet.opcode(), ArpOpCode::Request);
        assert_eq!(
            packet.sender_mac(),
            mac([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff])
        );
        assert_eq!(packet.sender_ip(), Ipv4Addr::new(192, 168, 1, 10));
        assert_eq!(packet.target_mac(), MacAddr::UNSPECIFIED);
        assert_eq!(packet.target_ip(), Ipv4Addr::new(192, 168, 1, 1));
    }

    #[test]
    fn decode_with_padding() {
        // NICs pad frames to the 60-byte minimum: 28 + 18 bytes of padding
        let mut buf = known_request_bytes();
        buf.extend_from_slice(&[0u8; 18]);
        let packet = ArpPacket::decode(ETHERTYPE_ARP, &buf).unwrap();
        assert_eq!(packet.target_ip(), Ipv4Addr::new(192, 168, 1, 1));
    }

    #[test]
    fn roundtrip() {
        let reply = sample(ArpOpCode::Reply, mac([0x11, 0x22, 0x33, 0x44, 0x55, 0x66]));
        let decoded = ArpPacket::decode(ETHERTYPE_ARP, &reply.encode()).unwrap();

        assert_eq!(decoded.opcode(), reply.opcode());
        assert_eq!(decoded.sender_mac(), reply.sender_mac());
        assert_eq!(decoded.sender_ip(), reply.sender_ip());
        assert_eq!(decoded.target_mac(), reply.target_mac());
        assert_eq!(decoded.target_ip(), reply.target_ip());
    }

    #[test]
    fn decode_rejects_too_short() {
        let buf = known_request_bytes();
        assert!(matches!(
            ArpPacket::decode(ETHERTYPE_ARP, &buf[..27]),
            Err(ArpPacketError::TooShort(27))
        ));
    }

    #[test]
    fn decode_rejects_not_arp() {
        let buf = known_request_bytes();
        assert!(matches!(
            ArpPacket::decode(ETHERTYPE_IPV4, &buf),
            Err(ArpPacketError::NotArp(0x0800))
        ));
    }

    #[test]
    fn decode_rejects_incompatible() {
        let mut buf = known_request_bytes();
        buf[0..2].copy_from_slice(&6u16.to_be_bytes()); // htype: IEEE 802
        assert!(matches!(
            ArpPacket::decode(ETHERTYPE_ARP, &buf),
            Err(ArpPacketError::EthIpv4(
                etherparse::err::arp::ArpEthIpv4FromError::NonMatchingHwType(_)
            ))
        ));
    }

    #[test]
    fn decode_rejects_bad_lengths() {
        // Trailing zeros so the buffer can hold the (bogus) announced lengths
        let mut buf = known_request_bytes();
        buf[4] = 8; // hlen
        buf.extend_from_slice(&[0u8; 64]);
        assert!(matches!(
            ArpPacket::decode(ETHERTYPE_ARP, &buf),
            Err(ArpPacketError::EthIpv4(
                etherparse::err::arp::ArpEthIpv4FromError::NonMatchingHwAddrSize(8)
            ))
        ));

        let mut buf = known_request_bytes();
        buf[5] = 16; // plen
        buf.extend_from_slice(&[0u8; 64]);
        assert!(matches!(
            ArpPacket::decode(ETHERTYPE_ARP, &buf),
            Err(ArpPacketError::EthIpv4(
                etherparse::err::arp::ArpEthIpv4FromError::NonMatchingProtoAddrSize(16)
            ))
        ));
    }

    #[test]
    fn decode_rejects_unknown_opcode() {
        let mut buf = known_request_bytes();
        buf[6..8].copy_from_slice(&3u16.to_be_bytes());
        assert!(matches!(
            ArpPacket::decode(ETHERTYPE_ARP, &buf),
            Err(ArpPacketError::UnknownOpcode(3))
        ));
    }

    #[test]
    fn gratuitous_detection() {
        // Gratuitous reply: sender IP == target IP, sender MAC == target MAC
        let packet = ArpPacket::new(
            ArpOpCode::Reply,
            mac([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]),
            Ipv4Addr::new(192, 168, 1, 10),
            mac([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]),
            Ipv4Addr::new(192, 168, 1, 10),
        );
        assert!(packet.is_gratuitous());

        // Gratuitous request: sender IP == target IP, target MAC unspecified
        let packet = ArpPacket::new(
            ArpOpCode::Request,
            mac([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]),
            Ipv4Addr::new(192, 168, 1, 10),
            MacAddr::UNSPECIFIED,
            Ipv4Addr::new(192, 168, 1, 10),
        );
        assert!(packet.is_gratuitous());

        // Normal ARP: different sender/target IPs
        let packet = ArpPacket::new(
            ArpOpCode::Request,
            mac([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]),
            Ipv4Addr::new(192, 168, 1, 10),
            MacAddr::UNSPECIFIED,
            Ipv4Addr::new(192, 168, 1, 1),
        );
        assert!(!packet.is_gratuitous());

        // Same IP but different (specified) MACs: not gratuitous
        let packet = sample(ArpOpCode::Reply, mac([0x11, 0x22, 0x33, 0x44, 0x55, 0x66]));
        assert!(!packet.is_gratuitous());
    }

    #[test]
    fn opcode_values() {
        assert_eq!(ArpOpCode::Request.as_u16(), 1);
        assert_eq!(ArpOpCode::Reply.as_u16(), 2);
        assert!(ArpOpCode::from_u16(1).is_ok());
        assert!(ArpOpCode::from_u16(2).is_ok());
        assert!(ArpOpCode::from_u16(3).is_err());
        assert!(!ArpOpCode::Request.is_reply());
        assert!(ArpOpCode::Reply.is_reply());
    }

    #[test]
    fn datalink_arp_roundtrip() {
        const DST: MacAddr = MacAddr::new([0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        const SRC: MacAddr = MacAddr::new([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);

        let frame: DataLinkFrame<ArpPacket> = DataLinkFrame::new(
            DST,
            SRC,
            ArpPacket::new(
                ArpOpCode::Request,
                SRC,
                Ipv4Addr::new(192, 168, 1, 10),
                MacAddr::UNSPECIFIED,
                Ipv4Addr::new(192, 168, 1, 1),
            ),
        );
        let decoded: DataLinkFrame<ArpPacket> = DataLinkFrame::decode(&frame.encode()).unwrap();

        assert_eq!(decoded.eth_dst, DST);
        assert_eq!(decoded.eth_src, SRC);
        assert_eq!(decoded.opcode(), ArpOpCode::Request);
        assert_eq!(decoded.sender_mac(), SRC);
        assert_eq!(decoded.sender_ip(), Ipv4Addr::new(192, 168, 1, 10));
        assert_eq!(decoded.target_ip(), Ipv4Addr::new(192, 168, 1, 1));
    }
}
