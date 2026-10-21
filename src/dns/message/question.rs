use super::error::DnsMessageError;
use crate::dns::decoder::DnsDecoder;
use crate::dns::encoder::DnsEncoder;
use crate::dns::name::DomainName;

/// Question section format
///
/// ```text
///                                     1  1  1  1  1  1
///       0  1  2  3  4  5  6  7  8  9  0  1  2  3  4  5
///     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
///     |                                               |
///     /                     QNAME                     /
///     /                                               /
///     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
///     |                     QTYPE                     |
///     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
///     |                     QCLASS                    |
///     +--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+--+
/// ```
///
/// <https://datatracker.ietf.org/doc/html/rfc1035#section-4.1.2>
#[derive(Debug)]
pub struct DnsQuestion {
    pub name: DomainName,
    pub qtype: u16,
    pub qclass: u16,
}

impl DnsQuestion {
    pub(super) fn write(&self, encoder: &mut DnsEncoder) {
        self.name.write(encoder);
        encoder.write_u16(self.qtype);
        encoder.write_u16(self.qclass);
    }

    pub(super) fn read(decoder: &mut DnsDecoder) -> Result<Self, DnsMessageError> {
        Ok(Self {
            name: DomainName::read(decoder)?,
            qtype: decoder.read_u16()?,
            qclass: decoder.read_u16()?,
        })
    }
}
