use std::net::Ipv4Addr;

use bytes::{BufMut, Bytes, BytesMut};
use libc::{ETH_P_ARP, ETH_P_IP};
use thiserror::Error;

use crate::net::{DataLinkPayload, MacAddr, MacAddrParseError};

const ETHERTYPE_ARP: u16 = ETH_P_ARP as u16;
const ETHERTYPE_IPV4: u16 = ETH_P_IP as u16;
const LEN: usize = 28;

const OPCODE_REQUEST: u16 = 1;
const OPCODE_REPLY: u16 = 2;

#[derive(Debug, Error)]
pub enum ArpPacketError {
    #[error(transparent)]
    MacAddr(#[from] MacAddrParseError),
    #[error("Frame too short")]
    TooShort(usize),
    #[error("Not ARP")]
    NotArp(u16),
    #[error("Incompatible hardware or protocol type")]
    Incompatible { htype: u16, ptype: u16 },
    #[error("HLEN/PLEN different from 6/4")]
    BadLengths { hlen: u8, plen: u8 },
    #[error("Unknown opcode")]
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

    fn as_u16(&self) -> u16 {
        match self {
            Self::Request => OPCODE_REQUEST,
            Self::Reply => OPCODE_REPLY,
        }
    }

    #[inline]
    pub fn is_reply(&self) -> bool {
        matches!(self, Self::Reply)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ArpPacket {
    pub opcode: ArpOpCode,
    pub sender_mac: MacAddr,
    pub sender_ip: Ipv4Addr,
    pub target_mac: MacAddr,
    pub target_ip: Ipv4Addr,
}

impl ArpPacket {
    /// Returns `true` if it's a gratuitous ARP.
    ///
    /// In a gratuitous ARP the sender IP and target IP are the same. The target
    /// MAC is the sender MAC in a gratuitous reply, and unspecified (all zeros)
    /// in a gratuitous request.
    #[inline]
    pub fn is_gratuitous(&self) -> bool {
        self.sender_ip == self.target_ip
            && (self.sender_mac == self.target_mac || self.target_mac.is_unspecified())
    }
}

impl DataLinkPayload for ArpPacket {
    type Error = ArpPacketError;

    fn ethertype(&self) -> u16 {
        ETHERTYPE_ARP
    }

    fn decode(ethertype: u16, buf: &[u8]) -> Result<Self, Self::Error> {
        if ethertype != ETHERTYPE_ARP {
            return Err(ArpPacketError::NotArp(ethertype));
        }

        if buf.len() < LEN {
            return Err(ArpPacketError::TooShort(buf.len()));
        }

        let htype = u16::from_be_bytes([buf[0], buf[1]]);
        let ptype = u16::from_be_bytes([buf[2], buf[3]]);
        if htype != 1 || ptype != ETHERTYPE_IPV4 {
            return Err(ArpPacketError::Incompatible { htype, ptype });
        }

        let (hlen, plen) = (buf[4], buf[5]);
        if hlen != 6 || plen != 4 {
            return Err(ArpPacketError::BadLengths { hlen, plen });
        }

        let opcode: u16 = u16::from_be_bytes([buf[6], buf[7]]);
        let opcode: ArpOpCode = ArpOpCode::from_u16(opcode)?;

        // Ignore trailing padding (NICs pad frames to the 60-byte minimum)
        Ok(Self {
            opcode,
            sender_mac: MacAddr::from_slice(&buf[8..14])?,
            sender_ip: Ipv4Addr::new(buf[14], buf[15], buf[16], buf[17]),
            target_mac: MacAddr::from_slice(&buf[18..24])?,
            target_ip: Ipv4Addr::new(buf[24], buf[25], buf[26], buf[27]),
        })
    }

    fn encode(&self) -> Bytes {
        let mut bytes: BytesMut = BytesMut::with_capacity(LEN);
        bytes.extend_from_slice(&1u16.to_be_bytes()); // Ethernet
        bytes.extend_from_slice(&ETHERTYPE_IPV4.to_be_bytes()); // IPv4
        bytes.put_u8(6); // HLEN
        bytes.put_u8(4); // PLEN
        bytes.extend_from_slice(&self.opcode.as_u16().to_be_bytes());
        bytes.extend_from_slice(self.sender_mac.as_bytes());
        bytes.extend_from_slice(&self.sender_ip.octets());
        bytes.extend_from_slice(self.target_mac.as_bytes());
        bytes.extend_from_slice(&self.target_ip.octets());
        bytes.freeze()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{DataLinkFrame, DataLinkPayload};

    fn mac(b: [u8; 6]) -> MacAddr {
        MacAddr::new(b)
    }

    fn sample(opcode: ArpOpCode, target_mac: MacAddr) -> ArpPacket {
        ArpPacket {
            opcode,
            sender_mac: mac([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]),
            sender_ip: Ipv4Addr::new(192, 168, 1, 10),
            target_mac,
            target_ip: Ipv4Addr::new(192, 168, 1, 1),
        }
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
        assert_eq!(packet.encode().len(), LEN);
    }

    #[test]
    fn decode_known_bytes() {
        let packet = ArpPacket::decode(ETHERTYPE_ARP, &known_request_bytes()).unwrap();
        assert_eq!(packet.opcode, ArpOpCode::Request);
        assert_eq!(packet.sender_mac, mac([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]));
        assert_eq!(packet.sender_ip, Ipv4Addr::new(192, 168, 1, 10));
        assert_eq!(packet.target_mac, MacAddr::UNSPECIFIED);
        assert_eq!(packet.target_ip, Ipv4Addr::new(192, 168, 1, 1));
    }

    #[test]
    fn decode_with_padding() {
        // NICs pad frames to the 60-byte minimum: 28 + 18 bytes of padding
        let mut buf = known_request_bytes();
        buf.extend_from_slice(&[0u8; 18]);
        let packet = ArpPacket::decode(ETHERTYPE_ARP, &buf).unwrap();
        assert_eq!(packet.target_ip, Ipv4Addr::new(192, 168, 1, 1));
    }

    #[test]
    fn roundtrip() {
        let reply = sample(ArpOpCode::Reply, mac([0x11, 0x22, 0x33, 0x44, 0x55, 0x66]));
        let decoded = ArpPacket::decode(ETHERTYPE_ARP, &reply.encode()).unwrap();

        assert_eq!(decoded.opcode, reply.opcode);
        assert_eq!(decoded.sender_mac, reply.sender_mac);
        assert_eq!(decoded.sender_ip, reply.sender_ip);
        assert_eq!(decoded.target_mac, reply.target_mac);
        assert_eq!(decoded.target_ip, reply.target_ip);
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
            Err(ArpPacketError::Incompatible {
                htype: 6,
                ptype: 0x0800
            })
        ));
    }

    #[test]
    fn decode_rejects_bad_lengths() {
        let mut buf = known_request_bytes();
        buf[4] = 8; // hlen
        assert!(matches!(
            ArpPacket::decode(ETHERTYPE_ARP, &buf),
            Err(ArpPacketError::BadLengths { hlen: 8, plen: 4 })
        ));

        let mut buf = known_request_bytes();
        buf[5] = 16; // plen
        assert!(matches!(
            ArpPacket::decode(ETHERTYPE_ARP, &buf),
            Err(ArpPacketError::BadLengths { hlen: 6, plen: 16 })
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
        let packet = ArpPacket {
            opcode: ArpOpCode::Reply,
            sender_mac: mac([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]),
            sender_ip: Ipv4Addr::new(192, 168, 1, 10),
            target_mac: mac([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]),
            target_ip: Ipv4Addr::new(192, 168, 1, 10),
        };
        assert!(packet.is_gratuitous());

        // Gratuitous request: sender IP == target IP, target MAC unspecified
        let packet = ArpPacket {
            opcode: ArpOpCode::Request,
            sender_mac: mac([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]),
            sender_ip: Ipv4Addr::new(192, 168, 1, 10),
            target_mac: MacAddr::UNSPECIFIED,
            target_ip: Ipv4Addr::new(192, 168, 1, 10),
        };
        assert!(packet.is_gratuitous());

        // Normal ARP: different sender/target IPs
        let packet = ArpPacket {
            opcode: ArpOpCode::Request,
            sender_mac: mac([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]),
            sender_ip: Ipv4Addr::new(192, 168, 1, 10),
            target_mac: MacAddr::UNSPECIFIED,
            target_ip: Ipv4Addr::new(192, 168, 1, 1),
        };
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
            ArpPacket {
                opcode: ArpOpCode::Request,
                sender_mac: SRC,
                sender_ip: Ipv4Addr::new(192, 168, 1, 10),
                target_mac: MacAddr::UNSPECIFIED,
                target_ip: Ipv4Addr::new(192, 168, 1, 1),
            },
        );
        let decoded: DataLinkFrame<ArpPacket> = DataLinkFrame::decode(&frame.encode()).unwrap();

        assert_eq!(decoded.eth_dst, DST);
        assert_eq!(decoded.eth_src, SRC);
        assert_eq!(decoded.opcode, ArpOpCode::Request);
        assert_eq!(decoded.sender_mac, SRC);
        assert_eq!(decoded.sender_ip, Ipv4Addr::new(192, 168, 1, 10));
        assert_eq!(decoded.target_ip, Ipv4Addr::new(192, 168, 1, 1));
    }
}
