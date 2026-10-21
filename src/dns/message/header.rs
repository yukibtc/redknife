use crate::dns::message::error::DnsMessageError;

#[derive(Debug, Clone, Copy)]
pub enum DnsOpcode {
    Query,
    Unknown(u8),
}

impl DnsOpcode {
    #[inline]
    fn from_u8(opcode: u8) -> Self {
        match opcode {
            0 => Self::Query,
            other => Self::Unknown(other),
        }
    }

    #[inline]
    fn to_u8(self) -> u8 {
        match self {
            Self::Query => 0,
            Self::Unknown(unknown) => unknown,
        }
    }
}

impl DnsFlags {
    #[inline]
    pub(super) fn decode(hi: u8, lo: u8) -> Self {
        Self {
            qr: hi & 0x80 != 0,
            opcode: DnsOpcode::from_u8((hi >> 3) & 0x0F),
            aa: hi & 0x04 != 0,
            tc: hi & 0x02 != 0,
            rd: hi & 0x01 != 0,
            ra: lo & 0x80 != 0,
            ad: lo & 0x20 != 0,
            cd: lo & 0x10 != 0,
            rcode: DnsRCode::from_u8(lo & 0x0F),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum DnsRCode {
    NoError,
    FormErr,
    ServFail,
    NXDomain,
    NotImp,
    Refused,
    Unknown(u8),
}

impl DnsRCode {
    #[inline]
    fn from_u8(opcode: u8) -> Self {
        match opcode {
            0 => Self::NoError,
            1 => Self::FormErr,
            2 => Self::ServFail,
            3 => Self::NXDomain,
            4 => Self::NotImp,
            5 => Self::Refused,
            other => Self::Unknown(other),
        }
    }

    #[inline]
    fn to_u8(self) -> u8 {
        match self {
            Self::NoError => 0,
            Self::FormErr => 1,
            Self::ServFail => 2,
            Self::NXDomain => 3,
            Self::NotImp => 4,
            Self::Refused => 5,
            Self::Unknown(unknown) => unknown,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DnsFlags {
    /// `true` if response, `false` if query
    pub qr: bool,
    pub opcode: DnsOpcode,
    /// Authoritative Answer
    ///
    /// `true` if the server "owns" the requested zone
    pub aa: bool,
    pub tc: bool,
    pub rd: bool,
    pub ra: bool,
    pub ad: bool,
    pub cd: bool,
    pub rcode: DnsRCode,
}

impl DnsFlags {
    #[cfg(test)]
    pub fn is_response(&self) -> bool {
        self.qr
    }

    //       0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5
    //     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
    //     |QR|   Opcode  |AA|TC|RD|RA|Z |AD|CD|   RCODE   |
    //     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
    #[inline]
    fn encode(&self) -> [u8; 2] {
        // hi part
        let qr: u8 = (self.qr as u8) << 7;
        let opcode: u8 = self.opcode.to_u8() << 3;
        let aa: u8 = (self.aa as u8) << 2;
        let tc: u8 = (self.tc as u8) << 1;
        let rd: u8 = self.rd as u8;
        let hi: u8 = qr | opcode | aa | tc | rd;

        // lo part
        let ra: u8 = (self.ra as u8) << 7;
        let ad: u8 = (self.ad as u8) << 5;
        let cd: u8 = (self.cd as u8) << 4;
        let rcode: u8 = self.rcode.to_u8();
        let lo: u8 = ra | ad | cd | rcode;

        [hi, lo]
    }
}

/// Header section format
///
/// ```text
///                                      1  1  1  1  1  1
///       0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5
///     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
///     |                      ID                       |
///     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
///     |QR|   Opcode  |AA|TC|RD|RA|   Z    |   RCODE   |
///     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
///     |                    QDCOUNT                    |
///     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
///     |                    ANCOUNT                    |
///     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
///     |                    NSCOUNT                    |
///     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
///     |                    ARCOUNT                    |
///     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
/// ```
///
/// <https://datatracker.ietf.org/doc/html/rfc1035#section-4.1.1>
#[derive(Debug, Clone, Copy)]
pub struct DnsHeader {
    pub id: u16,
    pub flags: DnsFlags,
    pub qdcount: u16,
    pub ancount: u16,
    pub nscount: u16,
    pub arcount: u16,
}

impl DnsHeader {
    pub(super) fn encode(&self) -> [u8; 12] {
        let id: [u8; 2] = self.id.to_be_bytes();
        let flags: [u8; 2] = self.flags.encode();
        let qdcount: [u8; 2] = self.qdcount.to_be_bytes();
        let ancount: [u8; 2] = self.ancount.to_be_bytes();
        let nscount: [u8; 2] = self.nscount.to_be_bytes();
        let arcount: [u8; 2] = self.arcount.to_be_bytes();
        [
            id[0], id[1], flags[0], flags[1], qdcount[0], qdcount[1], ancount[0], ancount[1],
            nscount[0], nscount[1], arcount[0], arcount[1],
        ]
    }

    pub(super) fn decode(bytes: &[u8]) -> Result<Self, DnsMessageError> {
        let bytes: [u8; 12] = bytes
            .try_into()
            .map_err(|_| DnsMessageError::UnexpectedEof)?;

        Ok(Self {
            id: u16::from_be_bytes([bytes[0], bytes[1]]),
            flags: DnsFlags::decode(bytes[2], bytes[3]),
            qdcount: u16::from_be_bytes([bytes[4], bytes[5]]),
            ancount: u16::from_be_bytes([bytes[6], bytes[7]]),
            nscount: u16::from_be_bytes([bytes[8], bytes[9]]),
            arcount: u16::from_be_bytes([bytes[10], bytes[11]]),
        })
    }
}
