use std::{fmt, io};

use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum LdapError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Decode(#[from] DecodeError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DecodeErrorReason {
    UnexpectedTag { expected: u8, actual: u8 },
    ExtendedBerTag { tag: u8 },
    IndefiniteLength,
    TooManyLengthOctets { count: usize, limit: usize },
    LengthOverflow,
    TruncatedElement,
    InvalidIntegerLength { length: usize },
    InvalidBoolean,
    InvalidUtf8,
    TrailingFields,
    InvalidMessageId { value: i32 },
    UnrecognizedAuthenticationTag { tag: u8 },
    UnrecognizedOperationTag { tag: u8 },
    InvalidAbandonTarget { value: i32 },
    NonemptyUnbind,
    InvalidSearchScope { value: i32 },
    InvalidAliasDereferencing { value: i32 },
    FilterDepthLimit { limit: usize },
    FilterNodeLimit { limit: usize },
    EmptyFilterSet,
    UnrecognizedFilterTag { tag: u8 },
    FrameTooLarge { declared: usize, limit: usize },
    BufferTooLarge { received: usize, limit: usize },
    IncompleteMessage { received: usize },
}

impl fmt::Display for DecodeErrorReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedTag { expected, actual } => write!(
                formatter,
                "unexpected BER tag 0x{actual:02x}, expected 0x{expected:02x}"
            ),
            Self::ExtendedBerTag { tag } => {
                write!(formatter, "unsupported extended BER tag 0x{tag:02x}")
            }
            Self::IndefiniteLength => formatter.write_str("indefinite BER length is not allowed"),
            Self::TooManyLengthOctets { count, limit } => write!(
                formatter,
                "BER length uses {count} octets, limit is {limit}"
            ),
            Self::LengthOverflow => formatter.write_str("BER length overflow"),
            Self::TruncatedElement => {
                formatter.write_str("truncated BER element inside a container")
            }
            Self::InvalidIntegerLength { length } => write!(
                formatter,
                "invalid LDAP integer length {length}, expected 1..=4 octets"
            ),
            Self::InvalidBoolean => formatter.write_str("invalid LDAP boolean encoding"),
            Self::InvalidUtf8 => formatter.write_str("LDAP string is not UTF-8"),
            Self::TrailingFields => formatter.write_str("unexpected trailing LDAP fields"),
            Self::InvalidMessageId { value } => {
                write!(
                    formatter,
                    "invalid message ID {value}, expected a positive integer"
                )
            }
            Self::UnrecognizedAuthenticationTag { tag } => {
                write!(
                    formatter,
                    "unrecognized constructed Bind authentication tag 0x{tag:02x}"
                )
            }
            Self::UnrecognizedOperationTag { tag } => {
                write!(formatter, "unrecognized LDAP operation tag 0x{tag:02x}")
            }
            Self::InvalidAbandonTarget { value } => {
                write!(formatter, "invalid Abandon target message ID {value}")
            }
            Self::NonemptyUnbind => formatter.write_str("Unbind content must be empty"),
            Self::InvalidSearchScope { value } => write!(formatter, "invalid Search scope {value}"),
            Self::InvalidAliasDereferencing { value } => {
                write!(formatter, "invalid alias dereferencing value {value}")
            }
            Self::FilterDepthLimit { limit } => {
                write!(formatter, "LDAP filter exceeds depth limit {limit}")
            }
            Self::FilterNodeLimit { limit } => {
                write!(formatter, "LDAP filter exceeds node limit {limit}")
            }
            Self::EmptyFilterSet => formatter.write_str("AND/OR filter set must not be empty"),
            Self::UnrecognizedFilterTag { tag } => {
                write!(formatter, "unrecognized LDAP filter tag 0x{tag:02x}")
            }
            Self::FrameTooLarge { declared, limit } => write!(
                formatter,
                "declared LDAP frame size {declared} exceeds limit {limit}"
            ),
            Self::BufferTooLarge { received, limit } => write!(
                formatter,
                "LDAP receive buffer size {received} exceeds limit {limit}"
            ),
            Self::IncompleteMessage { received } => {
                write!(
                    formatter,
                    "EOF inside an incomplete LDAP message ({received} bytes received)"
                )
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DecodeError {
    reason: DecodeErrorReason,
    message_id: Option<i32>,
    operation_tag: Option<u8>,
}

impl DecodeError {
    #[inline]
    pub(crate) fn new(reason: DecodeErrorReason) -> Self {
        Self {
            reason,
            message_id: None,
            operation_tag: None,
        }
    }

    #[inline]
    pub(crate) fn with_context(
        mut self,
        message_id: Option<i32>,
        operation_tag: Option<u8>,
    ) -> Self {
        self.message_id = message_id;
        self.operation_tag = operation_tag;

        self
    }
}

impl std::error::Error for DecodeError {}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.reason)?;

        if let Some(message_id) = self.message_id {
            write!(formatter, "; message={message_id}")?;
        }

        if let Some(operation_tag) = self.operation_tag {
            write!(formatter, " operation_tag=0x{operation_tag:02x}")?;
        }

        Ok(())
    }
}

impl From<DecodeErrorReason> for DecodeError {
    fn from(reason: DecodeErrorReason) -> Self {
        Self::new(reason)
    }
}
