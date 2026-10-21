use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

use super::error::DnsMessageError;
use crate::dns::DnsType;
use crate::dns::decoder::DnsDecoder;
use crate::dns::encoder::DnsEncoder;
use crate::dns::name::DomainName;

#[derive(Debug, PartialEq)]
pub enum DnsRecordData {
    A(Ipv4Addr),
    Aaaa(Ipv6Addr),
    Txt(Vec<String>),
    Ns(DomainName),
    Cname(DomainName),
    Ptr(DomainName),
    Mx {
        preference: u16,
        exchange: DomainName,
    },
    Soa {
        mname: DomainName,
        rname: DomainName,
        serial: u32,
        refresh: u32,
        retry: u32,
        expire: u32,
        minimum: u32,
    },
    Opt {
        payload_size: u16,
        ext_rcode: u8,
        version: u8,
        do_bit: bool,
    },
    Unknown {
        rtype: u16,
        data: Vec<u8>,
    },
}

impl fmt::Display for DnsRecordData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::A(ip) => write!(f, "{ip}"),
            Self::Aaaa(ip) => write!(f, "{ip}"),
            Self::Txt(strings) => {
                for (index, string) in strings.iter().enumerate() {
                    if index > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "\"{string}\"")?;
                }
                Ok(())
            }
            Self::Ns(name) => write!(f, "{name}"),
            Self::Cname(name) => write!(f, "{name}"),
            Self::Ptr(name) => write!(f, "{name}"),
            Self::Mx {
                preference,
                exchange,
            } => write!(f, "{preference} {exchange}"),
            Self::Soa {
                mname,
                rname,
                serial,
                refresh,
                retry,
                expire,
                minimum,
            } => write!(
                f,
                "{mname} {rname} {serial} {refresh} {retry} {expire} {minimum}"
            ),
            Self::Opt {
                payload_size,
                ext_rcode,
                version,
                do_bit,
            } => write!(
                f,
                "payload-size {payload_size}, ext-rcode {ext_rcode}, version {version}, do {do_bit}"
            ),
            Self::Unknown { rtype, data } => {
                write!(f, "{rtype} ")?;
                write!(f, "\\# {} ", data.len())?;

                for byte in data {
                    write!(f, "{byte:02x}")?;
                }

                Ok(())
            }
        }
    }
}

impl DnsRecordData {
    fn write(&self, encoder: &mut DnsEncoder) {
        let mut data: DnsEncoder = DnsEncoder::new();

        match self {
            Self::A(addr) => data.write_bytes(&addr.octets()),
            Self::Aaaa(addr) => data.write_bytes(&addr.octets()),
            Self::Txt(strings) => {
                for string in strings {
                    data.write_u8(string.len() as u8);
                    data.write_bytes(string.as_bytes());
                }
            }
            Self::Ns(name) | Self::Cname(name) | Self::Ptr(name) => name.write(&mut data),
            Self::Mx {
                preference,
                exchange,
            } => {
                data.write_u16(*preference);
                exchange.write(&mut data);
            }
            Self::Soa {
                mname,
                rname,
                serial,
                refresh,
                retry,
                expire,
                minimum,
            } => {
                mname.write(&mut data);
                rname.write(&mut data);
                data.write_u32(*serial);
                data.write_u32(*refresh);
                data.write_u32(*retry);
                data.write_u32(*expire);
                data.write_u32(*minimum);
            }
            Self::Opt { .. } => {}
            Self::Unknown { data: bytes, .. } => data.write_bytes(bytes),
        }

        let bytes = data.finish();

        encoder.write_u16(bytes.len() as u16);
        encoder.write_bytes(&bytes);
    }

    pub(super) fn read(
        decoder: &mut DnsDecoder,
        rtype: DnsType,
        class: u16,
        ttl: u32,
        rdlength: usize,
    ) -> Result<Self, DnsMessageError> {
        let start: usize = decoder.offset();

        let rdata: Self = match rtype {
            DnsType::A => Self::A(Ipv4Addr::new(
                decoder.read_u8()?,
                decoder.read_u8()?,
                decoder.read_u8()?,
                decoder.read_u8()?,
            )),
            DnsType::Aaaa => {
                let bytes: [u8; 16] = decoder
                    .read_bytes(16)?
                    .try_into()
                    .map_err(|_| DnsMessageError::UnexpectedEof)?;
                Self::Aaaa(Ipv6Addr::from(bytes))
            }
            DnsType::Txt => {
                let mut strings: Vec<String> = Vec::new();
                let mut remaining: usize = rdlength;

                while remaining > 0 {
                    let length: usize = decoder.read_u8()? as usize;
                    remaining = remaining
                        .checked_sub(length + 1)
                        .ok_or(DnsMessageError::InvalidRData("TXT"))?;
                    let bytes: &[u8] = decoder.read_bytes(length)?;
                    let string = String::from_utf8_lossy(bytes);
                    strings.push(string.into_owned());
                }

                Self::Txt(strings)
            }
            DnsType::Ns => Self::Ns(DomainName::read(decoder)?),
            DnsType::Cname => Self::Cname(DomainName::read(decoder)?),
            DnsType::Ptr => Self::Ptr(DomainName::read(decoder)?),
            DnsType::Mx => Self::Mx {
                preference: decoder.read_u16()?,
                exchange: DomainName::read(decoder)?,
            },
            DnsType::Soa => Self::Soa {
                mname: DomainName::read(decoder)?,
                rname: DomainName::read(decoder)?,
                serial: decoder.read_u32()?,
                refresh: decoder.read_u32()?,
                retry: decoder.read_u32()?,
                expire: decoder.read_u32()?,
                minimum: decoder.read_u32()?,
            },
            DnsType::Opt => Self::Opt {
                payload_size: class,
                ext_rcode: (ttl >> 24) as u8,
                version: (ttl >> 16) as u8,
                do_bit: ttl & 0x8000 != 0,
            },
            DnsType::Unknown(rtype) => Self::Unknown {
                rtype,
                data: decoder.read_bytes(rdlength)?.to_vec(),
            },
        };

        let consumed: usize = decoder.offset() - start;

        if consumed > rdlength {
            return Err(DnsMessageError::InvalidRData("length mismatch"));
        }

        decoder.skip(rdlength - consumed)?;

        Ok(rdata)
    }
}

/// Resource record format
///
/// ```text
///                                     1  1  1  1  1  1
///       0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5
///     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
///     |                                               |
///     /                                               /
///     /                      NAME                     /
///     |                                               |
///     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
///     |                      TYPE                     |
///     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
///     |                     CLASS                     |
///     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
///     |                      TTL                      |
///     |                                               |
///     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
///     |                   RDLENGTH                    |
///     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--|
///     /                     RDATA                     /
///     /                                               /
///     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
///
/// <https://datatracker.ietf.org/doc/html/rfc1035#section-4.1.3>
#[derive(Debug, PartialEq)]
pub struct DnsRecord {
    pub name: DomainName,
    pub r#type: DnsType,
    pub class: u16,
    pub ttl: u32,
    pub rdata: DnsRecordData,
}

impl DnsRecord {
    pub(super) fn write(&self, encoder: &mut DnsEncoder) {
        self.name.write(encoder);
        encoder.write_u16(self.r#type.to_u16());
        encoder.write_u16(self.class);
        encoder.write_u32(self.ttl);
        self.rdata.write(encoder);
    }

    pub(super) fn read(decoder: &mut DnsDecoder) -> Result<Self, DnsMessageError> {
        let name: DomainName = DomainName::read(decoder)?;
        let r#type: DnsType = DnsType::from_u16(decoder.read_u16()?);
        let class: u16 = decoder.read_u16()?;
        let ttl: u32 = decoder.read_u32()?;
        let rdlength: usize = decoder.read_u16()? as usize;
        let rdata: DnsRecordData = DnsRecordData::read(decoder, r#type, class, ttl, rdlength)?;

        Ok(Self {
            name,
            r#type,
            class,
            ttl,
            rdata,
        })
    }
}
