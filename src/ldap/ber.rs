//! Bounded BER primitives for the listener's LDAP wire subset.

use std::{io, str};

use bytes::BytesMut;

use super::error::{DecodeError, DecodeErrorReason};

pub(super) const BOOLEAN_TAG: u8 = 0x01;
pub(super) const INTEGER_TAG: u8 = 0x02;
pub(super) const OCTET_STRING_TAG: u8 = 0x04;
pub(super) const ENUMERATED_TAG: u8 = 0x0a;
pub(super) const SEQUENCE_TAG: u8 = 0x30;
pub(super) const SET_TAG: u8 = 0x31;

pub(super) const MAX_MESSAGE_SIZE: usize = 32 * 1024;
const MAX_LENGTH_OCTETS: usize = 4;
const LONG_FORM_LENGTH_FLAG: u8 = 0x80;
const LENGTH_OCTET_COUNT_MASK: u8 = 0x7f;
const TAG_NUMBER_MASK: u8 = 0x1f;
pub(super) const INTEGER_SIGN_BIT: u8 = 0x80;
pub(super) const CONSTRUCTED_FLAG: u8 = 0x20;

pub(super) struct BerHeader {
    tag: u8,
    header_len: usize,
    content_len: usize,
}

impl BerHeader {
    pub(super) fn decode(bytes: &[u8]) -> Result<Option<Self>, DecodeError> {
        let Some(tag) = bytes.first().copied() else {
            return Ok(None);
        };

        if tag & TAG_NUMBER_MASK == TAG_NUMBER_MASK {
            return Err(DecodeErrorReason::ExtendedBerTag { tag }.into());
        }

        let Some(length_marker) = bytes.get(1).copied() else {
            return Ok(None);
        };

        if length_marker & LONG_FORM_LENGTH_FLAG == 0 {
            return Ok(Some(Self {
                tag,
                header_len: 2,
                content_len: usize::from(length_marker),
            }));
        }

        let length_octet_count: usize = usize::from(length_marker & LENGTH_OCTET_COUNT_MASK);

        if length_octet_count == 0 {
            return Err(DecodeErrorReason::IndefiniteLength.into());
        }

        if length_octet_count > MAX_LENGTH_OCTETS {
            return Err(DecodeErrorReason::TooManyLengthOctets {
                count: length_octet_count,
                limit: MAX_LENGTH_OCTETS,
            }
            .into());
        }

        let header_len: usize = 2 + length_octet_count;

        let Some(length_octets) = bytes.get(2..header_len) else {
            return Ok(None);
        };

        let mut content_len: usize = 0;

        for octet in length_octets {
            let shifted_length: usize = content_len
                .checked_mul(256)
                .ok_or_else(|| DecodeError::new(DecodeErrorReason::LengthOverflow))?;
            content_len = shifted_length
                .checked_add(usize::from(*octet))
                .ok_or_else(|| DecodeError::new(DecodeErrorReason::LengthOverflow))?;
        }

        Ok(Some(Self {
            tag,
            header_len,
            content_len,
        }))
    }

    #[inline]
    pub(super) fn header_len(&self) -> usize {
        self.header_len
    }

    #[inline]
    pub(super) fn total_len(&self) -> Result<usize, DecodeError> {
        self.header_len
            .checked_add(self.content_len)
            .ok_or_else(|| DecodeError::new(DecodeErrorReason::LengthOverflow))
    }
}

pub(super) struct BerElement<'a> {
    pub(super) tag: u8,
    pub(super) content: &'a [u8],
}

impl BerElement<'_> {
    pub(super) fn decode_integer(self) -> Result<i32, DecodeError> {
        let bytes: &[u8] = self.content;

        if !(1..=4).contains(&bytes.len()) {
            return Err(DecodeErrorReason::InvalidIntegerLength {
                length: bytes.len(),
            }
            .into());
        }

        // BER INTEGER is signed. Extend the first octet's sign to a full i32 rather
        // than shifting a larger integer and silently truncating its high bits.
        let sign_extension: u8 = if bytes[0] & INTEGER_SIGN_BIT == 0 {
            0
        } else {
            0xff
        };
        let mut encoded: [u8; 4] = [sign_extension; 4];
        let content_start: usize = encoded.len() - bytes.len();

        encoded[content_start..].copy_from_slice(bytes);

        Ok(i32::from_be_bytes(encoded))
    }

    #[inline]
    pub(super) fn decode_string(self) -> Result<String, DecodeError> {
        str::from_utf8(self.content)
            .map(str::to_owned)
            .map_err(|_| DecodeErrorReason::InvalidUtf8.into())
    }
}

/// A cursor over one complete LDAP container.
pub(super) struct LdapReader<'a> {
    remaining: &'a [u8],
}

impl<'a> LdapReader<'a> {
    #[inline]
    pub(super) fn new(content: &'a [u8]) -> Self {
        Self { remaining: content }
    }

    #[inline]
    pub(super) fn is_empty(&self) -> bool {
        self.remaining.is_empty()
    }

    #[inline]
    pub(super) fn peek_tag(&self) -> Option<u8> {
        self.remaining.first().copied()
    }

    pub(super) fn read_element(&mut self) -> Result<BerElement<'a>, DecodeError> {
        let header: BerHeader = BerHeader::decode(self.remaining)?
            .ok_or_else(|| DecodeError::new(DecodeErrorReason::TruncatedElement))?;
        let element_len: usize = header.total_len()?;

        let content: &[u8] = self
            .remaining
            .get(header.header_len..element_len)
            .ok_or_else(|| DecodeError::new(DecodeErrorReason::TruncatedElement))?;

        self.remaining = &self.remaining[element_len..];

        Ok(BerElement {
            tag: header.tag,
            content,
        })
    }

    pub(super) fn read_bytes(&mut self, expected_tag: u8) -> Result<&'a [u8], DecodeError> {
        let element: BerElement = self.read_element()?;

        if element.tag != expected_tag {
            return Err(DecodeErrorReason::UnexpectedTag {
                expected: expected_tag,
                actual: element.tag,
            }
            .into());
        }

        Ok(element.content)
    }

    pub(super) fn read_integer(&mut self, expected_tag: u8) -> Result<i32, DecodeError> {
        let element: BerElement = self.read_element()?;

        if element.tag != expected_tag {
            return Err(DecodeErrorReason::UnexpectedTag {
                expected: expected_tag,
                actual: element.tag,
            }
            .into());
        }

        element.decode_integer()
    }

    pub(super) fn read_boolean(&mut self) -> Result<bool, DecodeError> {
        // LDAP encodes TRUE as FF and FALSE as 00, each exactly one octet.
        match self.read_bytes(BOOLEAN_TAG)? {
            [0] => Ok(false),
            [0xff] => Ok(true),
            _ => Err(DecodeErrorReason::InvalidBoolean.into()),
        }
    }

    pub(super) fn read_string(&mut self) -> Result<String, DecodeError> {
        let element: BerElement = self.read_element()?;

        if element.tag != OCTET_STRING_TAG {
            return Err(DecodeErrorReason::UnexpectedTag {
                expected: OCTET_STRING_TAG,
                actual: element.tag,
            }
            .into());
        }

        element.decode_string()
    }

    pub(super) fn finish(self) -> Result<(), DecodeError> {
        if !self.is_empty() {
            return Err(DecodeErrorReason::TrailingFields.into());
        }

        Ok(())
    }
}

#[inline]
pub(super) fn malformed() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "malformed LDAP message")
}

pub(super) fn write_element(output: &mut BytesMut, tag: u8, content: &[u8]) -> io::Result<()> {
    if content.len() > MAX_MESSAGE_SIZE {
        return Err(malformed());
    }

    output.extend_from_slice(&[tag]);
    write_length(output, content.len());
    output.extend_from_slice(content);

    Ok(())
}

fn write_length(output: &mut BytesMut, content_len: usize) {
    if content_len < usize::from(LONG_FORM_LENGTH_FLAG) {
        output.extend_from_slice(&[content_len as u8]);
    } else if content_len <= usize::from(u8::MAX) {
        output.extend_from_slice(&[LONG_FORM_LENGTH_FLAG | 1, content_len as u8]);
    } else {
        output.extend_from_slice(&[
            LONG_FORM_LENGTH_FLAG | 2,
            (content_len >> 8) as u8,
            content_len as u8,
        ]);
    }
}
