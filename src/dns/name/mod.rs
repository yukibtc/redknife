use std::fmt;
use std::str::FromStr;

use thiserror::Error;

mod label;

use self::label::Label;
pub(super) use self::label::LabelError;
use super::decoder::DnsDecoder;
use super::encoder::DnsEncoder;
use super::message::DnsMessageError;

const MAX_POINTERS: usize = 128;
const DOMAIN_NAME_MAX_LENGTH: usize = 256;

#[derive(Debug, PartialEq, Eq, Error)]
pub enum DomainNameError {
    #[error("Domain name is too big: {DOMAIN_NAME_MAX_LENGTH} <= {0}")]
    DomainNameLength(usize),
    #[error("{0}")]
    LabelError(#[from] LabelError),
}

/// Domain name.
///
/// <https://datatracker.ietf.org/doc/html/rfc2181#section-11>
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DomainName {
    labels: Vec<Label>,
}

impl DomainName {
    #[inline]
    pub fn root() -> Self {
        Self { labels: Vec::new() }
    }

    fn append_label(&mut self, label: Label) -> Result<(), DomainNameError> {
        let label_length: usize = label.len() as usize;

        let domain_name_length: usize = if self.labels.is_empty() {
            label_length + 1
        } else {
            self.len() + label_length + 1
        };

        if DOMAIN_NAME_MAX_LENGTH <= domain_name_length {
            return Err(DomainNameError::DomainNameLength(domain_name_length));
        }

        self.labels.push(label);

        Ok(())
    }

    pub fn len(&self) -> usize {
        let mut len: usize = self.labels.len();

        if len == 0 {
            return 1;
        }

        for label in self.labels.iter() {
            len += label.len() as usize;
        }

        len
    }

    #[inline]
    pub fn is_root(&self) -> bool {
        self.labels.is_empty()
    }

    fn from_labels(labels: Vec<Label>) -> Result<Self, DomainNameError> {
        let mut name: Self = Self::root();

        for label in labels {
            name.append_label(label)?;
        }

        Ok(name)
    }

    /// Read a possibly-compressed domain name, RFC 1035 section 4.1.4
    pub(super) fn read(decoder: &mut DnsDecoder) -> Result<Self, DnsMessageError> {
        let mut labels: Vec<Label> = Vec::new();
        let mut jumps: usize = 0;
        let mut pos: usize = decoder.offset();
        let mut end: Option<usize> = None;

        loop {
            let length: u8 = decoder.peek_u8_at(pos)?;

            match length & 0xC0 {
                0x00 => {
                    if length == 0 {
                        let end: usize = end.unwrap_or(pos + 1);
                        decoder.seek(end);

                        return Ok(Self::from_labels(labels)?);
                    }

                    let start: usize = pos + 1;
                    let stop: usize = start + length as usize;
                    let bytes: &[u8] = decoder.slice_at(start, stop)?;

                    labels.push(Label::from_bytes(bytes)?);

                    pos = stop;
                }
                0xC0 => {
                    let second: u8 = decoder.peek_u8_at(pos + 1)?;
                    let offset: usize = (((length & 0x3F) as usize) << 8) | second as usize;

                    if offset >= pos {
                        return Err(DnsMessageError::InvalidPointer(pos));
                    }

                    if end.is_none() {
                        end = Some(pos + 2);
                    }

                    jumps += 1;

                    if jumps > MAX_POINTERS {
                        return Err(DnsMessageError::TooManyPointers);
                    }

                    pos = offset;
                }
                _ => return Err(DnsMessageError::InvalidPointer(pos)),
            }
        }
    }

    pub(super) fn write(&self, encoder: &mut DnsEncoder) {
        for label in &self.labels {
            encoder.write_u8(label.len());
            encoder.write_bytes(label.as_bytes());
        }

        encoder.write_u8(0);
    }
}

impl fmt::Display for DomainName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_root() {
            write!(f, ".")
        } else {
            for label in self.labels.iter() {
                write!(f, "{label}.")?;
            }
            Ok(())
        }
    }
}

impl FromStr for DomainName {
    type Err = DomainNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value: &str = value.strip_suffix('.').unwrap_or(value);

        let mut name: Self = Self::root();

        for label in value.split('.') {
            let label: Label = label.parse()?;
            name.append_label(label)?;
        }

        Ok(name)
    }
}
