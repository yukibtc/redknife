use std::{fmt, io, str};

use bytes::BytesMut;

use super::ber::{
    BOOLEAN_TAG, BerElement, CONSTRUCTED_FLAG, ENUMERATED_TAG, INTEGER_SIGN_BIT, INTEGER_TAG,
    LdapReader, MAX_MESSAGE_SIZE, OCTET_STRING_TAG, SEQUENCE_TAG, SET_TAG, malformed,
    write_element,
};
use super::error::{DecodeError, DecodeErrorReason};

pub(super) const MAX_FILTER_DEPTH: usize = 128;
pub(super) const MAX_FILTER_NODES: usize = 1024;

const BIND_REQUEST_TAG: u8 = 0x60;
const BIND_RESPONSE_TAG: u8 = 0x61;
const UNBIND_REQUEST_TAG: u8 = 0x42;
const SEARCH_REQUEST_TAG: u8 = 0x63;
const SEARCH_ENTRY_TAG: u8 = 0x64;
const SEARCH_DONE_TAG: u8 = 0x65;
const MODIFY_REQUEST_TAG: u8 = 0x66;
const MODIFY_RESPONSE_TAG: u8 = 0x67;
const ADD_REQUEST_TAG: u8 = 0x68;
const ADD_RESPONSE_TAG: u8 = 0x69;
const DELETE_REQUEST_TAG: u8 = 0x4a;
const DELETE_RESPONSE_TAG: u8 = 0x6b;
const MODIFY_DN_REQUEST_TAG: u8 = 0x6c;
const MODIFY_DN_RESPONSE_TAG: u8 = 0x6d;
const COMPARE_REQUEST_TAG: u8 = 0x6e;
const COMPARE_RESPONSE_TAG: u8 = 0x6f;
const ABANDON_REQUEST_TAG: u8 = 0x50;
const EXTENDED_REQUEST_TAG: u8 = 0x77;
const EXTENDED_RESPONSE_TAG: u8 = 0x78;

const SIMPLE_AUTHENTICATION_TAG: u8 = 0x80;
const SASL_AUTHENTICATION_TAG: u8 = 0xa3;
const SASL_CHALLENGE_TAG: u8 = 0x87;
const SICILY_DISCOVERY_TAG: u8 = 0x89;
const SICILY_NEGOTIATE_TAG: u8 = 0x8a;
const SICILY_RESPONSE_TAG: u8 = 0x8b;
const CONTROLS_TAG: u8 = 0xa0;
const AND_FILTER_TAG: u8 = 0xa0;
const OR_FILTER_TAG: u8 = 0xa1;
const NOT_FILTER_TAG: u8 = 0xa2;
const EQUALITY_FILTER_TAG: u8 = 0xa3;
const SUBSTRING_FILTER_TAG: u8 = 0xa4;
const GREATER_OR_EQUAL_FILTER_TAG: u8 = 0xa5;
const LESS_OR_EQUAL_FILTER_TAG: u8 = 0xa6;
const PRESENT_FILTER_TAG: u8 = 0x87;
const APPROXIMATE_FILTER_TAG: u8 = 0xa8;
const EXTENSIBLE_FILTER_TAG: u8 = 0xa9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum ResultCode {
    Success = 0,
    OperationsError = 1,
    ProtocolError = 2,
    AuthMethodNotSupported = 7,
    UnavailableCriticalExtension = 12,
    SaslBindInProgress = 14,
    NoSuchObject = 32,
    InvalidDNSyntax = 34,
    UnwillingToPerform = 53,
}

impl ResultCode {
    #[inline]
    fn encode(self, output: &mut BytesMut) -> io::Result<()> {
        write_element(output, ENUMERATED_TAG, &[self as u8])
    }
}

pub(super) struct Request {
    pub message_id: i32,
    pub operation: RequestOperation,
    pub critical_control: bool,
}

impl Request {
    pub(super) fn decode(content: &[u8]) -> Result<Self, DecodeError> {
        let mut reader: LdapReader = LdapReader::new(content);

        let message_id: i32 = reader.read_integer(INTEGER_TAG)?;
        let operation_tag: Option<u8> = reader.peek_tag();

        Self::decode_fields(reader, message_id)
            .map_err(|error| error.with_context(Some(message_id), operation_tag))
    }

    fn decode_fields(mut reader: LdapReader, message_id: i32) -> Result<Self, DecodeError> {
        // Message ID zero is reserved for server notifications.
        if message_id <= 0 {
            return Err(DecodeErrorReason::InvalidMessageId { value: message_id }.into());
        }

        let element: BerElement = reader.read_element()?;
        let operation: RequestOperation = RequestOperation::decode(element)?;

        let critical_control: bool = if reader.is_empty() {
            false
        } else {
            let controls_content: &[u8] = reader.read_bytes(CONTROLS_TAG)?;
            Self::decode_controls(controls_content)?
        };

        reader.finish()?;

        Ok(Self {
            message_id,
            operation,
            critical_control,
        })
    }

    fn decode_controls(content: &[u8]) -> Result<bool, DecodeError> {
        let mut reader: LdapReader = LdapReader::new(content);
        let mut has_critical_control: bool = false;

        while !reader.is_empty() {
            let control_content: &[u8] = reader.read_bytes(SEQUENCE_TAG)?;
            let mut control: LdapReader = LdapReader::new(control_content);

            control.read_string()?;

            if control.peek_tag() == Some(BOOLEAN_TAG) {
                has_critical_control |= control.read_boolean()?;
            }

            if !control.is_empty() {
                control.read_bytes(OCTET_STRING_TAG)?;
            }

            control.finish()?;
        }

        Ok(has_critical_control)
    }
}

pub(super) enum RequestOperation {
    Bind(BindRequest),
    Search(SearchRequest),
    Unbind,
    Abandon,
    Unsupported(UnsupportedOperation),
}

impl RequestOperation {
    fn decode(element: BerElement) -> Result<Self, DecodeError> {
        match element.tag {
            BIND_REQUEST_TAG => Ok(Self::Bind(BindRequest::decode(element.content)?)),
            SEARCH_REQUEST_TAG => Ok(Self::Search(SearchRequest::decode(element.content)?)),
            UNBIND_REQUEST_TAG => {
                if !element.content.is_empty() {
                    return Err(DecodeErrorReason::NonemptyUnbind.into());
                }

                Ok(Self::Unbind)
            }
            ABANDON_REQUEST_TAG => {
                let target_message_id: i32 = element.decode_integer()?;

                if target_message_id <= 0 {
                    return Err(DecodeErrorReason::InvalidAbandonTarget {
                        value: target_message_id,
                    }
                    .into());
                }

                Ok(Self::Abandon)
            }
            DELETE_REQUEST_TAG
            | MODIFY_REQUEST_TAG
            | ADD_REQUEST_TAG
            | MODIFY_DN_REQUEST_TAG
            | COMPARE_REQUEST_TAG
            | EXTENDED_REQUEST_TAG => Ok(Self::Unsupported(UnsupportedOperation::decode(element)?)),
            _ => Err(DecodeErrorReason::UnrecognizedOperationTag { tag: element.tag }.into()),
        }
    }
}

#[derive(Debug)]
pub(crate) struct BindRequest {
    pub version: i32,
    pub dn: String,
    pub authentication: BindAuthentication,
}

impl BindRequest {
    fn decode(content: &[u8]) -> Result<Self, DecodeError> {
        let mut reader: LdapReader = LdapReader::new(content);

        // The version is validated by session policy, mirroring slapd's reply
        // order, so even LDAPv2 Bind credentials are captured and displayed.
        let version: i32 = reader.read_integer(INTEGER_TAG)?;
        let dn: String = reader.read_string()?;
        let credentials: BerElement = reader.read_element()?;

        let authentication: BindAuthentication = BindAuthentication::decode(credentials)?;

        reader.finish()?;

        Ok(Self {
            version,
            dn,
            authentication,
        })
    }
}

pub(crate) enum BindAuthentication {
    Simple(Vec<u8>),
    Sicily(SicilyAuthentication),
    Sasl {
        mechanism: SaslMechanism,
        credentials: Vec<u8>,
    },
    Unknown {
        tag: u8,
        credentials: Vec<u8>,
    },
}

impl fmt::Debug for BindAuthentication {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Simple(_) => formatter.write_str("Simple(<redacted>)"),
            Self::Sasl { mechanism, .. } => formatter
                .debug_struct("Sasl")
                .field("mechanism", mechanism)
                .field("credentials", &"<redacted>")
                .finish(),
            Self::Sicily(authentication) => authentication.fmt(formatter),
            Self::Unknown { tag, .. } => formatter
                .debug_struct("Unknown")
                .field("tag", tag)
                .field("credentials", &"<redacted>")
                .finish(),
        }
    }
}

impl BindAuthentication {
    fn decode(element: BerElement) -> Result<Self, DecodeError> {
        match element.tag {
            SIMPLE_AUTHENTICATION_TAG => Ok(Self::Simple(element.content.to_vec())),
            SASL_AUTHENTICATION_TAG => Self::decode_sasl(element.content),
            SICILY_DISCOVERY_TAG | SICILY_NEGOTIATE_TAG | SICILY_RESPONSE_TAG => {
                Ok(Self::Sicily(SicilyAuthentication::decode(element)?))
            }
            tag if tag & CONSTRUCTED_FLAG == 0 => Ok(Self::Unknown {
                tag,
                credentials: element.content.to_vec(),
            }),
            _ => Err(DecodeErrorReason::UnrecognizedAuthenticationTag { tag: element.tag }.into()),
        }
    }

    fn decode_sasl(content: &[u8]) -> Result<Self, DecodeError> {
        let mut reader: LdapReader = LdapReader::new(content);

        let mechanism_name: String = reader.read_string()?;
        let mechanism: SaslMechanism = mechanism_name.into();

        // RFC 4511 permits an omitted initial SASL response. Session policy treats
        // absent and empty responses equally when deciding whether to challenge.
        let credentials: Vec<u8> = if reader.is_empty() {
            Vec::new()
        } else {
            reader.read_bytes(OCTET_STRING_TAG)?.to_vec()
        };

        reader.finish()?;

        Ok(Self::Sasl {
            mechanism,
            credentials,
        })
    }
}

pub(crate) enum SicilyAuthentication {
    PackageDiscovery(Vec<u8>),
    Negotiate(Vec<u8>),
    Response(Vec<u8>),
}

impl fmt::Debug for SicilyAuthentication {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind: &str = match self {
            Self::PackageDiscovery(_) => "SicilyPackageDiscovery",
            Self::Negotiate(_) => "SicilyNegotiate",
            Self::Response(_) => "SicilyResponse",
        };

        write!(formatter, "{kind}(<redacted>)")
    }
}

impl SicilyAuthentication {
    fn decode(element: BerElement) -> Result<Self, DecodeError> {
        let credentials: Vec<u8> = element.content.to_vec();

        match element.tag {
            SICILY_DISCOVERY_TAG => Ok(Self::PackageDiscovery(credentials)),
            SICILY_NEGOTIATE_TAG => Ok(Self::Negotiate(credentials)),
            SICILY_RESPONSE_TAG => Ok(Self::Response(credentials)),
            _ => Err(DecodeErrorReason::UnrecognizedAuthenticationTag { tag: element.tag }.into()),
        }
    }

    pub(crate) fn payload(&self) -> &[u8] {
        match self {
            Self::PackageDiscovery(payload)
            | Self::Negotiate(payload)
            | Self::Response(payload) => payload,
        }
    }
}

pub(crate) struct PlainCredentials<'a> {
    pub authorization_id: &'a str,
    pub authentication_id: &'a str,
    pub password: &'a [u8],
}

impl<'a> PlainCredentials<'a> {
    pub(crate) fn decode(credentials: &'a [u8]) -> Option<Self> {
        let mut fields = credentials.split(|byte| *byte == 0);
        let authorization_bytes: &[u8] = fields.next()?;
        let authentication_bytes: &[u8] = fields.next()?;
        let password: &[u8] = fields.next()?;

        if fields.next().is_some() || authentication_bytes.is_empty() || password.is_empty() {
            return None;
        }

        let authorization_id: &str = str::from_utf8(authorization_bytes).ok()?;
        let authentication_id: &str = str::from_utf8(authentication_bytes).ok()?;

        Some(Self {
            authorization_id,
            authentication_id,
            password,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SaslMechanism {
    Plain,
    Login,
    Other(String),
}

impl From<String> for SaslMechanism {
    fn from(name: String) -> Self {
        match name.as_str() {
            "PLAIN" => Self::Plain,
            "LOGIN" => Self::Login,
            _ => Self::Other(name),
        }
    }
}

pub(super) struct SearchRequest {
    pub base: String,
    pub scope: SearchScope,
    pub size_limit: i32,
    pub time_limit: i32,
    pub types_only: bool,
    pub filter: Filter,
    pub attributes: Vec<String>,
}

impl SearchRequest {
    fn decode(content: &[u8]) -> Result<Self, DecodeError> {
        // Fields follow the SearchRequest wire order from RFC 4511, section 4.5.1.
        let mut reader: LdapReader = LdapReader::new(content);

        let base: String = reader.read_string()?;
        let encoded_scope: i32 = reader.read_integer(ENUMERATED_TAG)?;
        let scope: SearchScope = SearchScope::decode(encoded_scope)?;
        let dereference_aliases: i32 = reader.read_integer(ENUMERATED_TAG)?;

        // The RootDSE has no aliases, but the field must still have a valid value.
        if !(0..=3).contains(&dereference_aliases) {
            return Err(DecodeErrorReason::InvalidAliasDereferencing {
                value: dereference_aliases,
            }
            .into());
        }

        let size_limit: i32 = reader.read_integer(INTEGER_TAG)?;
        let time_limit: i32 = reader.read_integer(INTEGER_TAG)?;
        let types_only: bool = reader.read_boolean()?;

        // Depth limits stack use; a separate node budget also bounds wide filters.
        let mut remaining_nodes: usize = MAX_FILTER_NODES;
        let filter: Filter = Filter::decode(&mut reader, MAX_FILTER_DEPTH, &mut remaining_nodes)?;

        let selectors_content: &[u8] = reader.read_bytes(SEQUENCE_TAG)?;
        let mut selectors_reader: LdapReader = LdapReader::new(selectors_content);
        let mut attributes: Vec<String> = Vec::new();

        while !selectors_reader.is_empty() {
            attributes.push(selectors_reader.read_string()?);
        }

        reader.finish()?;

        Ok(Self {
            base,
            scope,
            size_limit,
            time_limit,
            types_only,
            filter,
            attributes,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum SearchScope {
    Base = 0,
    OneLevel = 1,
    Subtree = 2,
    Children = 3,
}

impl SearchScope {
    fn decode(encoded_scope: i32) -> Result<Self, DecodeError> {
        match encoded_scope {
            0 => Ok(Self::Base),
            1 => Ok(Self::OneLevel),
            2 => Ok(Self::Subtree),
            3 => Ok(Self::Children),
            _ => Err(DecodeErrorReason::InvalidSearchScope {
                value: encoded_scope,
            }
            .into()),
        }
    }
}

pub(super) enum Filter {
    Present(String),
    Equality(String, Vec<u8>),
    And(Vec<Filter>),
    Or(Vec<Filter>),
    Not(Box<Filter>),
    Unsupported,
}

impl Filter {
    fn decode(
        reader: &mut LdapReader,
        remaining_depth: usize,
        remaining_nodes: &mut usize,
    ) -> Result<Self, DecodeError> {
        if remaining_depth == 0 {
            return Err(DecodeErrorReason::FilterDepthLimit {
                limit: MAX_FILTER_DEPTH,
            }
            .into());
        }

        if *remaining_nodes == 0 {
            return Err(DecodeErrorReason::FilterNodeLimit {
                limit: MAX_FILTER_NODES,
            }
            .into());
        }

        *remaining_nodes -= 1;

        let element: BerElement = reader.read_element()?;

        match element.tag {
            PRESENT_FILTER_TAG => Ok(Self::Present(element.decode_string()?)),
            EQUALITY_FILTER_TAG => {
                let mut assertion: LdapReader = LdapReader::new(element.content);
                let name: String = assertion.read_string()?;
                let value: Vec<u8> = assertion.read_bytes(OCTET_STRING_TAG)?.to_vec();

                assertion.finish()?;

                Ok(Self::Equality(name, value))
            }
            AND_FILTER_TAG => {
                let children: Vec<Self> =
                    Self::decode_children(element.content, remaining_depth - 1, remaining_nodes)?;

                Ok(Self::And(children))
            }
            OR_FILTER_TAG => {
                let children: Vec<Self> =
                    Self::decode_children(element.content, remaining_depth - 1, remaining_nodes)?;

                Ok(Self::Or(children))
            }
            NOT_FILTER_TAG => {
                let mut child_reader: LdapReader = LdapReader::new(element.content);
                let child: Self =
                    Self::decode(&mut child_reader, remaining_depth - 1, remaining_nodes)?;

                // NOT contains exactly one filter, unlike AND and OR.
                child_reader.finish()?;

                Ok(Self::Not(Box::new(child)))
            }
            SUBSTRING_FILTER_TAG
            | GREATER_OR_EQUAL_FILTER_TAG
            | LESS_OR_EQUAL_FILTER_TAG
            | APPROXIMATE_FILTER_TAG
            | EXTENSIBLE_FILTER_TAG => {
                validate_rejected_operands(element.content)?;

                Ok(Self::Unsupported)
            }
            _ => Err(DecodeErrorReason::UnrecognizedFilterTag { tag: element.tag }.into()),
        }
    }

    fn decode_children(
        content: &[u8],
        remaining_depth: usize,
        remaining_nodes: &mut usize,
    ) -> Result<Vec<Self>, DecodeError> {
        let mut reader: LdapReader = LdapReader::new(content);
        let mut children: Vec<Self> = Vec::new();

        while !reader.is_empty() {
            children.push(Self::decode(&mut reader, remaining_depth, remaining_nodes)?);
        }

        // LDAP's AND/OR filter sets must contain at least one child.
        if children.is_empty() {
            return Err(DecodeErrorReason::EmptyFilterSet.into());
        }

        Ok(children)
    }
}

#[derive(Clone, Copy)]
pub(super) enum UnsupportedOperation {
    Modify,
    Add,
    Delete,
    ModifyDn,
    Compare,
    Extended,
}

impl UnsupportedOperation {
    fn decode(element: BerElement) -> Result<Self, DecodeError> {
        if element.tag == DELETE_REQUEST_TAG {
            element.decode_string()?;
            return Ok(Self::Delete);
        }

        validate_rejected_operands(element.content)?;

        match element.tag {
            MODIFY_REQUEST_TAG => Ok(Self::Modify),
            ADD_REQUEST_TAG => Ok(Self::Add),
            MODIFY_DN_REQUEST_TAG => Ok(Self::ModifyDn),
            COMPARE_REQUEST_TAG => Ok(Self::Compare),
            EXTENDED_REQUEST_TAG => Ok(Self::Extended),
            _ => Err(DecodeErrorReason::UnrecognizedOperationTag { tag: element.tag }.into()),
        }
    }

    fn response_tag(self) -> u8 {
        match self {
            Self::Modify => MODIFY_RESPONSE_TAG,
            Self::Add => ADD_RESPONSE_TAG,
            Self::Delete => DELETE_RESPONSE_TAG,
            Self::ModifyDn => MODIFY_DN_RESPONSE_TAG,
            Self::Compare => COMPARE_RESPONSE_TAG,
            Self::Extended => EXTENDED_RESPONSE_TAG,
        }
    }
}

pub(super) struct Response {
    pub message_id: i32,
    pub operation: ResponseOperation,
}

impl Response {
    pub(super) fn encode(self) -> io::Result<BytesMut> {
        let mut content: BytesMut = BytesMut::new();
        Self::write_message_id(&mut content, self.message_id)?;

        let operation: BytesMut = self.operation.encode()?;
        content.extend_from_slice(&operation);

        let mut message: BytesMut = BytesMut::new();
        write_element(&mut message, SEQUENCE_TAG, &content)?;

        if message.len() > MAX_MESSAGE_SIZE {
            return Err(malformed());
        }

        Ok(message)
    }

    fn write_message_id(output: &mut BytesMut, message_id: i32) -> io::Result<()> {
        if message_id <= 0 {
            return Err(malformed());
        }

        let encoded: [u8; 4] = message_id.to_be_bytes();
        let mut first_octet: usize = 0;

        while first_octet < encoded.len() - 1 {
            let redundant_zero: bool = encoded[first_octet] == 0;
            let next_has_sign_bit: bool = encoded[first_octet + 1] & INTEGER_SIGN_BIT != 0;

            if !redundant_zero || next_has_sign_bit {
                break;
            }

            first_octet += 1;
        }

        write_element(output, INTEGER_TAG, &encoded[first_octet..])
    }
}

pub(super) enum ResponseOperation {
    Bind {
        code: ResultCode,
        challenge: Option<Vec<u8>>,
        diagnostic: Option<&'static str>,
    },
    SearchEntry(Vec<Attribute>),
    Result {
        kind: ResultKind,
        code: ResultCode,
    },
}

impl ResponseOperation {
    fn encode(self) -> io::Result<BytesMut> {
        let mut encoded: BytesMut = BytesMut::new();

        match self {
            Self::Bind {
                code,
                challenge,
                diagnostic,
            } => {
                let content: BytesMut = Self::encode_bind(code, challenge, diagnostic)?;
                write_element(&mut encoded, BIND_RESPONSE_TAG, &content)?;
            }
            Self::SearchEntry(attributes) => {
                let content: BytesMut = Self::encode_root_entry(attributes)?;
                write_element(&mut encoded, SEARCH_ENTRY_TAG, &content)?;
            }
            Self::Result { kind, code } => {
                let mut content: BytesMut = BytesMut::new();
                Self::write_result(&mut content, code, None)?;
                write_element(&mut encoded, kind.tag(), &content)?;
            }
        }

        Ok(encoded)
    }

    fn encode_bind(
        code: ResultCode,
        challenge: Option<Vec<u8>>,
        diagnostic: Option<&'static str>,
    ) -> io::Result<BytesMut> {
        let mut content: BytesMut = BytesMut::new();

        Self::write_result(&mut content, code, diagnostic)?;

        if let Some(challenge) = challenge {
            write_element(&mut content, SASL_CHALLENGE_TAG, &challenge)?;
        }

        Ok(content)
    }

    fn encode_root_entry(attributes: Vec<Attribute>) -> io::Result<BytesMut> {
        let mut entry: BytesMut = BytesMut::new();

        // The only returned entry is the RootDSE, whose DN is the empty string.
        write_element(&mut entry, OCTET_STRING_TAG, b"")?;

        let mut encoded_attributes: BytesMut = BytesMut::new();

        for attribute in attributes {
            let encoded_attribute: BytesMut = attribute.encode()?;
            encoded_attributes.extend_from_slice(&encoded_attribute);
        }

        write_element(&mut entry, SEQUENCE_TAG, &encoded_attributes)?;

        Ok(entry)
    }

    fn write_result(
        output: &mut BytesMut,
        code: ResultCode,
        diagnostic: Option<&'static str>,
    ) -> io::Result<()> {
        code.encode(output)?;

        write_element(output, OCTET_STRING_TAG, b"")?;
        write_element(
            output,
            OCTET_STRING_TAG,
            diagnostic.unwrap_or_default().as_bytes(),
        )
    }
}

#[derive(Clone, Copy)]
pub(super) enum ResultKind {
    SearchDone,
    Rejected(UnsupportedOperation),
}

impl ResultKind {
    fn tag(self) -> u8 {
        match self {
            Self::SearchDone => SEARCH_DONE_TAG,
            Self::Rejected(operation) => operation.response_tag(),
        }
    }
}

pub(super) struct Attribute {
    pub name: String,
    pub values: Vec<Vec<u8>>,
}

impl Attribute {
    fn encode(self) -> io::Result<BytesMut> {
        let mut content: BytesMut = BytesMut::new();
        write_element(&mut content, OCTET_STRING_TAG, self.name.as_bytes())?;

        let mut values: BytesMut = BytesMut::new();

        for value in self.values {
            write_element(&mut values, OCTET_STRING_TAG, &value)?;
        }

        write_element(&mut content, SET_TAG, &values)?;

        let mut encoded: BytesMut = BytesMut::new();
        write_element(&mut encoded, SEQUENCE_TAG, &content)?;

        Ok(encoded)
    }
}

fn validate_rejected_operands(content: &[u8]) -> Result<(), DecodeError> {
    // These operations/filters always receive a rejection. Check immediate TLV
    // boundaries only; their nested directory data is never interpreted or
    // allocated as a generic ASN.1 tree.
    let mut reader: LdapReader = LdapReader::new(content);

    while !reader.is_empty() {
        reader.read_element()?;
    }

    Ok(())
}
