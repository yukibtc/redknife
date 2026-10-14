use bytes::{Buf, BytesMut};

use super::ber::{BerHeader, LdapReader};
use super::protocol::{
    Attribute, BindAuthentication, BindRequest, Filter, RequestOperation, Response,
    ResponseOperation, ResultCode, ResultKind, SaslMechanism, SearchRequest, SicilyAuthentication,
    UnsupportedOperation,
};

pub(super) fn encode_element(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut encoded: Vec<u8> = vec![tag];
    let content_len: usize = content.len();

    if content_len < 128 {
        encoded.push(content_len as u8);
    } else {
        let length_octets: [u8; 8] = (content_len as u64).to_be_bytes();
        let first_octet: usize = length_octets
            .iter()
            .position(|octet| *octet != 0)
            .expect("long-form length is nonzero");

        let octet_count: usize = length_octets.len() - first_octet;

        encoded.push(0x80 | octet_count as u8);
        encoded.extend_from_slice(&length_octets[first_octet..]);
    }

    encoded.extend_from_slice(content);

    encoded
}

fn encode_integer(tag: u8, value: i32) -> Vec<u8> {
    let encoded: [u8; 4] = value.to_be_bytes();
    let mut first_octet: usize = 0;

    // Remove redundant sign extension, retaining the octet that makes the
    // remaining INTEGER positive or negative as appropriate.
    while first_octet < encoded.len() - 1 {
        let next_has_sign_bit: bool = encoded[first_octet + 1] >= 128;
        let positive_padding: bool = encoded[first_octet] == 0 && !next_has_sign_bit;
        let negative_padding: bool = encoded[first_octet] == 255 && next_has_sign_bit;

        if !positive_padding && !negative_padding {
            break;
        }

        first_octet += 1;
    }

    encode_element(tag, &encoded[first_octet..])
}

pub(super) fn encode_message(message_id: i32, operation: &[u8]) -> Vec<u8> {
    let mut content: Vec<u8> = encode_integer(0x02, message_id);
    content.extend_from_slice(operation);

    encode_element(0x30, &content)
}

pub(super) fn encode_request(message_id: i32, request: RequestOperation) -> Vec<u8> {
    let operation: Vec<u8> = match request {
        RequestOperation::Bind(bind) => encode_bind_request(bind),
        RequestOperation::Search(search) => encode_search_request(search),
        RequestOperation::Unbind => encode_element(0x42, b""),
        RequestOperation::Abandon => encode_integer(0x50, 1),
        RequestOperation::Unsupported(UnsupportedOperation::Extended) => {
            // LDAP StartTLS ExtendedRequest, with implicitly tagged requestName.
            let request_name: Vec<u8> = encode_element(0x80, b"1.3.6.1.4.1.1466.20037");

            encode_element(0x77, &request_name)
        }
        RequestOperation::Unsupported(
            UnsupportedOperation::Modify
            | UnsupportedOperation::Add
            | UnsupportedOperation::Delete
            | UnsupportedOperation::ModifyDn
            | UnsupportedOperation::Compare,
        ) => panic!("test helper has no fixture for this operation"),
    };

    encode_message(message_id, &operation)
}

fn encode_bind_request(bind: BindRequest) -> Vec<u8> {
    let mut content: Vec<u8> = encode_integer(0x02, bind.version);
    content.extend(encode_element(0x04, bind.dn.as_bytes()));

    let authentication: Vec<u8> = match bind.authentication {
        BindAuthentication::Simple(password) => encode_element(0x80, &password),
        BindAuthentication::Sicily(authentication) => match authentication {
            SicilyAuthentication::PackageDiscovery(credentials) => {
                encode_element(0x89, &credentials)
            }
            SicilyAuthentication::Negotiate(credentials) => encode_element(0x8a, &credentials),
            SicilyAuthentication::Response(credentials) => encode_element(0x8b, &credentials),
        },
        BindAuthentication::Unknown { tag, credentials } => encode_element(tag, &credentials),
        BindAuthentication::Sasl {
            mechanism,
            credentials,
        } => {
            let mechanism_name: &[u8] = match &mechanism {
                SaslMechanism::Plain => b"PLAIN",
                SaslMechanism::Login => b"LOGIN",
                SaslMechanism::Other(name) => name.as_bytes(),
            };

            let mut sasl: Vec<u8> = encode_element(0x04, mechanism_name);
            sasl.extend(encode_element(0x04, &credentials));

            encode_element(0xa3, &sasl)
        }
    };

    content.extend(authentication);

    encode_element(0x60, &content)
}

fn encode_search_request(search: SearchRequest) -> Vec<u8> {
    let mut content: Vec<u8> = encode_element(0x04, search.base.as_bytes());
    content.extend(encode_integer(0x0a, i32::from(search.scope as u8)));
    content.extend(encode_integer(0x0a, 0)); // Never dereference aliases.
    content.extend(encode_integer(0x02, search.size_limit));
    content.extend(encode_integer(0x02, search.time_limit));

    let types_only: u8 = if search.types_only { 255 } else { 0 };
    content.extend(encode_element(0x01, &[types_only]));
    let Filter::Present(name) = search.filter else {
        panic!("test helper only supports presence filters");
    };
    content.extend(encode_element(0x87, name.as_bytes()));

    let mut selectors: Vec<u8> = Vec::new();

    for name in search.attributes {
        selectors.extend(encode_element(0x04, name.as_bytes()));
    }

    content.extend(encode_element(0x30, &selectors));

    encode_element(0x63, &content)
}

pub(super) fn decode_response(input: &mut BytesMut) -> Option<Response> {
    let header: BerHeader = BerHeader::decode(input).expect("invalid response header")?;
    let message_len: usize = header.total_len().expect("response length overflow");

    if input.len() < message_len {
        return None;
    }

    let mut frame: LdapReader = LdapReader::new(&input[..message_len]);
    let mut envelope: LdapReader = LdapReader::new(frame.read_bytes(0x30).unwrap());

    let message_id: i32 = envelope.read_integer(0x02).unwrap();
    assert!(message_id >= 0, "negative response message ID");
    let operation_tag: u8 = envelope.peek_tag().expect("response operation is missing");
    let mut content: LdapReader = LdapReader::new(envelope.read_bytes(operation_tag).unwrap());

    let operation: ResponseOperation = match operation_tag {
        0x64 => ResponseOperation::SearchEntry(decode_root_attributes(&mut content)),
        0x61 => {
            let code: ResultCode = decode_result_code(&mut content);

            assert!(
                content.read_bytes(0x04).unwrap().is_empty(),
                "matchedDN must be empty"
            );

            let diagnostic: Option<&'static str> = decode_diagnostic(&mut content);
            let challenge: Option<Vec<u8>> = if content.is_empty() {
                None
            } else {
                Some(content.read_bytes(0x87).unwrap().to_vec())
            };

            ResponseOperation::Bind {
                code,
                challenge,
                diagnostic,
            }
        }
        _ => {
            let kind: ResultKind = decode_result_kind(operation_tag);
            let code: ResultCode = decode_result(&mut content);

            ResponseOperation::Result { kind, code }
        }
    };

    content.finish().unwrap();
    envelope.finish().unwrap();
    frame.finish().unwrap();

    input.advance(message_len);

    Some(Response {
        message_id,
        operation,
    })
}

fn decode_root_attributes(entry: &mut LdapReader<'_>) -> Vec<Attribute> {
    assert!(
        entry.read_bytes(0x04).unwrap().is_empty(),
        "RootDSE DN must be empty"
    );

    let mut list: LdapReader = LdapReader::new(entry.read_bytes(0x30).unwrap());
    let mut attributes: Vec<Attribute> = Vec::new();

    while !list.is_empty() {
        let mut item: LdapReader = LdapReader::new(list.read_bytes(0x30).unwrap());
        let name: String = item.read_string().unwrap();

        let mut set: LdapReader = LdapReader::new(item.read_bytes(0x31).unwrap());
        let mut values: Vec<Vec<u8>> = Vec::new();

        while !set.is_empty() {
            values.push(set.read_bytes(0x04).unwrap().to_vec());
        }

        item.finish().unwrap();

        attributes.push(Attribute { name, values });
    }

    attributes
}

fn decode_result_code(content: &mut LdapReader<'_>) -> ResultCode {
    match content.read_integer(0x0a).unwrap() {
        0 => ResultCode::Success,
        1 => ResultCode::OperationsError,
        2 => ResultCode::ProtocolError,
        7 => ResultCode::AuthMethodNotSupported,
        12 => ResultCode::UnavailableCriticalExtension,
        14 => ResultCode::SaslBindInProgress,
        32 => ResultCode::NoSuchObject,
        34 => ResultCode::InvalidDNSyntax,
        53 => ResultCode::UnwillingToPerform,
        _ => panic!("unexpected result code"),
    }
}

fn decode_diagnostic(content: &mut LdapReader<'_>) -> Option<&'static str> {
    let diagnostic: &[u8] = content.read_bytes(0x04).unwrap();

    match diagnostic {
        b"" => None,
        b"invalid DN" => Some("invalid DN"),
        b"unknown authentication method" => Some("unknown authentication method"),
        b"requested protocol version not supported" => {
            Some("requested protocol version not supported")
        }
        b"historical protocol version requested, use LDAPv3 instead" => {
            Some("historical protocol version requested, use LDAPv3 instead")
        }
        b"critical extension is not recognized" => Some("critical extension is not recognized"),
        _ => panic!("unexpected diagnostic message"),
    }
}

fn decode_result(content: &mut LdapReader<'_>) -> ResultCode {
    let code: ResultCode = decode_result_code(content);

    assert!(
        content.read_bytes(0x04).unwrap().is_empty(),
        "matchedDN must be empty"
    );
    assert!(
        content.read_bytes(0x04).unwrap().is_empty(),
        "diagnosticMessage must be empty"
    );

    code
}

fn decode_result_kind(tag: u8) -> ResultKind {
    match tag {
        0x65 => ResultKind::SearchDone,
        0x67 => ResultKind::Rejected(UnsupportedOperation::Modify),
        0x69 => ResultKind::Rejected(UnsupportedOperation::Add),
        0x6b => ResultKind::Rejected(UnsupportedOperation::Delete),
        0x6d => ResultKind::Rejected(UnsupportedOperation::ModifyDn),
        0x6f => ResultKind::Rejected(UnsupportedOperation::Compare),
        0x78 => ResultKind::Rejected(UnsupportedOperation::Extended),
        _ => panic!("unexpected result tag"),
    }
}
