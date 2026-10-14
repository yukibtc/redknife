//! TCP framing and Tokio codec adapters for the LDAP listener.

use std::io;

use bytes::{Buf, Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use super::ber::{BerHeader, MAX_MESSAGE_SIZE, SEQUENCE_TAG};
use super::error::{DecodeError, DecodeErrorReason, LdapError};
use super::event::RawMessage;
use super::protocol::{Request, Response};

const MAX_BUFFER_SIZE: usize = MAX_MESSAGE_SIZE * 2;

pub(super) struct ReceivedMessage {
    pub request: Result<Request, DecodeError>,
    pub raw: Option<RawMessage>,
}

#[derive(Default)]
pub(super) struct ServerCodec {
    raw: bool,
}

impl ServerCodec {
    #[inline]
    pub(super) fn new(raw: bool) -> Self {
        Self { raw }
    }

    pub(super) fn decode_message(
        &mut self,
        input: &mut BytesMut,
    ) -> Result<Option<ReceivedMessage>, LdapError> {
        if input.len() > MAX_BUFFER_SIZE {
            let reason: DecodeErrorReason = DecodeErrorReason::BufferTooLarge {
                received: input.len(),
                limit: MAX_BUFFER_SIZE,
            };

            return Err(DecodeError::new(reason).into());
        }

        // Reject an invalid envelope tag even if only its first byte arrived.
        match input.first().copied() {
            Some(SEQUENCE_TAG) | None => {}
            Some(actual) => {
                let reason: DecodeErrorReason = DecodeErrorReason::UnexpectedTag {
                    expected: SEQUENCE_TAG,
                    actual,
                };

                return Err(DecodeError::new(reason).into());
            }
        }

        let Some(header) = BerHeader::decode(input)? else {
            return Ok(None);
        };

        let message_len: usize = header.total_len()?;

        // Reject the declared size before waiting for or allocating the body.
        if message_len > MAX_MESSAGE_SIZE {
            let reason: DecodeErrorReason = DecodeErrorReason::FrameTooLarge {
                declared: message_len,
                limit: MAX_MESSAGE_SIZE,
            };

            return Err(DecodeError::new(reason).into());
        }

        let Some(content) = input.get(header.header_len()..message_len) else {
            return Ok(None);
        };

        let request: Result<Request, DecodeError> = Request::decode(content);
        let raw: Option<RawMessage> = if self.raw {
            Some(RawMessage {
                bytes: Bytes::copy_from_slice(&input[..message_len]),
            })
        } else {
            None
        };

        // Preserve incomplete/invalid input and leave coalesced frames intact.
        if request.is_ok() {
            input.advance(message_len);
        }

        Ok(Some(ReceivedMessage { request, raw }))
    }
}

impl Encoder<Response> for ServerCodec {
    type Error = io::Error;

    fn encode(&mut self, response: Response, output: &mut BytesMut) -> io::Result<()> {
        let encoded: BytesMut = response.encode()?;

        // Append only after full validation, preserving prior output on failure.
        output.extend_from_slice(&encoded);

        Ok(())
    }
}

impl Decoder for ServerCodec {
    type Item = Request;
    type Error = LdapError;

    fn decode(&mut self, input: &mut BytesMut) -> Result<Option<Request>, LdapError> {
        let Some(message) = self.decode_message(input)? else {
            return Ok(None);
        };

        Ok(Some(message.request?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::protocol::{
        Attribute, BindAuthentication, BindRequest, Filter, MAX_FILTER_DEPTH, MAX_FILTER_NODES,
        RequestOperation, ResponseOperation, ResultCode, ResultKind, SaslMechanism, SearchScope,
        SicilyAuthentication,
    };
    use crate::ldap::test_wire;

    // LDAPMessage ID 1: LDAPv3 Simple Bind, DN "alice", password "secret".
    const SIMPLE_BIND: &[u8] =
        b"\x30\x17\x02\x01\x01\x60\x12\x02\x01\x03\x04\x05alice\x80\x06secret";

    // LDAPMessage ID 1: SASL LOGIN with the optional credentials field omitted.
    const LOGIN_WITHOUT_CREDENTIALS: &[u8] =
        b"\x30\x13\x02\x01\x01\x60\x0e\x02\x01\x03\x04\x00\xa3\x07\x04\x05LOGIN";

    // Base-scope RootDSE search, filter (objectClass=*), selector "+".
    const ROOT_DSE_SEARCH: &[u8] =
        b"\x30\x28\x02\x01\x01\x63\x23\x04\x00\x0a\x01\x00\x0a\x01\x00\x02\x01\x00\x02\x01\x00\x01\x01\x00\x87\x0bobjectClass\x30\x03\x04\x01+";

    fn decode_complete_packet(packet: &[u8]) -> Request {
        let mut input: BytesMut = BytesMut::from(packet);

        let request: Request = ServerCodec::default()
            .decode(&mut input)
            .expect("fixture must be valid")
            .expect("fixture must contain a complete message");

        assert!(input.is_empty(), "fixture must contain exactly one message");

        request
    }

    #[test]
    fn raw_recording_preserves_a_complete_frame_alongside_its_decode_error() {
        let mut packet: Vec<u8> = SIMPLE_BIND.to_vec();

        packet[17] = 0xa0;

        for raw in [false, true] {
            let mut codec: ServerCodec = ServerCodec::new(raw);
            let mut input: BytesMut = BytesMut::from(packet.as_slice());
            let message: ReceivedMessage = codec.decode_message(&mut input).unwrap().unwrap();

            assert!(message.request.is_err());
            assert_eq!(message.raw.is_some(), raw);
            assert_eq!(input.as_ref(), packet);

            if let Some(frame) = message.raw {
                assert_eq!(frame.bytes.as_ref(), packet);
            }
        }
    }

    #[test]
    fn sicily_authentication_choices_preserve_binary_tokens() {
        for tag in [0x89, 0x8a, 0x8b] {
            let token: &[u8] = b"\0\xff\x80binary-token";
            let authentication: Vec<u8> = test_wire::encode_element(tag, token);
            let mut content: Vec<u8> = b"\x02\x01\x03\x04\x00".to_vec();
            content.extend_from_slice(&authentication);

            let operation: Vec<u8> = test_wire::encode_element(0x60, &content);
            let packet: Vec<u8> = test_wire::encode_message(33, &operation);

            let request: Request = decode_complete_packet(&packet);

            assert_eq!(request.message_id, 33);

            let RequestOperation::Bind(bind) = request.operation else {
                panic!("expected BindRequest");
            };
            let BindAuthentication::Sicily(authentication) = bind.authentication else {
                panic!("expected Sicily authentication");
            };

            let (actual_tag, actual_token) = match authentication {
                SicilyAuthentication::PackageDiscovery(token) => (0x89, token),
                SicilyAuthentication::Negotiate(token) => (0x8a, token),
                SicilyAuthentication::Response(token) => (0x8b, token),
            };

            assert_eq!(actual_tag, tag);
            assert_eq!(actual_token, token);
        }
    }

    fn assert_invalid_packet(packet: &[u8], case: &str) -> DecodeError {
        let mut input: BytesMut = BytesMut::from(packet);

        let error: LdapError = ServerCodec::default()
            .decode(&mut input)
            .err()
            .unwrap_or_else(|| panic!("{case}: packet must be rejected"));

        assert_eq!(&input[..], packet, "{case}: error must not consume input");

        let LdapError::Decode(diagnostic) = error else {
            panic!("{case}: packet must produce a decoding failure");
        };

        diagnostic
    }

    fn encode_reply(message_id: i32, operation: ResponseOperation) -> BytesMut {
        let response: Response = Response {
            message_id,
            operation,
        };
        let mut encoded: BytesMut = BytesMut::new();

        ServerCodec::default()
            .encode(response, &mut encoded)
            .unwrap();

        encoded
    }

    fn root_search_packet(encoded_filter: &[u8]) -> Vec<u8> {
        // Empty base, base scope, no alias dereferencing, unlimited size/time,
        // typesOnly=false; filter and attribute selection follow these fields.
        let mut fields: Vec<u8> =
            b"\x04\x00\x0a\x01\x00\x0a\x01\x00\x02\x01\x00\x02\x01\x00\x01\x01\x00".to_vec();

        fields.extend_from_slice(encoded_filter);
        fields.extend_from_slice(b"\x30\x00"); // Empty attribute selector list.

        let operation: Vec<u8> = test_wire::encode_element(0x63, &fields);

        test_wire::encode_message(1, &operation)
    }

    #[test]
    fn independent_request_fixtures_decode_the_expected_fields() {
        let simple: Request = decode_complete_packet(SIMPLE_BIND);

        assert_eq!(simple.message_id, 1);
        assert!(!simple.critical_control);

        let RequestOperation::Bind(bind) = simple.operation else {
            panic!("expected Simple Bind");
        };

        let BindAuthentication::Simple(password) = bind.authentication else {
            panic!("expected Simple authentication");
        };

        assert_eq!(bind.version, 3);
        assert_eq!(bind.dn, "alice");
        assert_eq!(password, b"secret");

        let unknown: Request =
            decode_complete_packet(b"\x30\x0f\x02\x01\x03\x60\x0a\x02\x01\x03\x04\x00\x8c\x03xyz");
        assert_eq!(unknown.message_id, 3);
        let RequestOperation::Bind(bind) = unknown.operation else {
            panic!("expected BindRequest");
        };
        let BindAuthentication::Unknown { tag, credentials } = bind.authentication else {
            panic!("expected unknown primitive authentication");
        };
        assert_eq!(tag, 0x8c);
        assert_eq!(credentials, b"xyz");

        let login: Request = decode_complete_packet(LOGIN_WITHOUT_CREDENTIALS);

        let RequestOperation::Bind(bind) = login.operation else {
            panic!("expected SASL Bind");
        };

        let BindAuthentication::Sasl {
            mechanism,
            credentials,
        } = bind.authentication
        else {
            panic!("expected SASL authentication");
        };

        assert_eq!(mechanism, SaslMechanism::Login);
        assert!(credentials.is_empty());

        let lowercase: Request = decode_complete_packet(
            b"\x30\x13\x02\x01\x01\x60\x0e\x02\x01\x03\x04\x00\xa3\x07\x04\x05plain",
        );
        let RequestOperation::Bind(bind) = lowercase.operation else {
            panic!("expected BindRequest");
        };
        let BindAuthentication::Sasl {
            mechanism,
            credentials,
        } = bind.authentication
        else {
            panic!("expected SASL authentication");
        };

        assert_eq!(mechanism, SaslMechanism::Other("plain".to_owned()));
        assert!(credentials.is_empty());

        let root: Request = decode_complete_packet(ROOT_DSE_SEARCH);

        let RequestOperation::Search(search) = root.operation else {
            panic!("expected RootDSE search");
        };

        assert!(search.base.is_empty());
        assert_eq!(search.scope, SearchScope::Base);
        assert!(!search.types_only);
        assert_eq!(search.attributes, vec!["+"]);
        assert!(matches!(search.filter, Filter::Present(name) if name == "objectClass"));

        // OR of a binary equality assertion, presence, and unsupported greaterOrEqual.
        let packet: Vec<u8> = root_search_packet(
            b"\xa1\x1f\xa3\x08\x04\x02cn\x04\x02\x00\xff\x87\x0bobjectClass\xa5\x06\x04\x01x\x04\x01y",
        );
        let request: Request = decode_complete_packet(&packet);
        let RequestOperation::Search(search) = request.operation else {
            panic!("expected SearchRequest");
        };
        let Filter::Or(children) = search.filter else {
            panic!("expected OR filter");
        };
        let [
            Filter::Equality(name, value),
            Filter::Present(present),
            Filter::Unsupported,
        ] = children.as_slice()
        else {
            panic!("expected equality, presence, and unsupported filter children");
        };

        assert_eq!(name, "cn");
        assert_eq!(value, b"\x00\xff");
        assert_eq!(present, "objectClass");
    }

    #[test]
    fn every_fragment_boundary_preserves_incomplete_input() {
        let mut fixtures: Vec<Vec<u8>> = vec![
            SIMPLE_BIND.to_vec(),
            ROOT_DSE_SEARCH.to_vec(),
            LOGIN_WITHOUT_CREDENTIALS.to_vec(),
        ];

        // Also exercise split points inside one- and two-octet length headers.
        for password_len in [128, 256] {
            let request: RequestOperation = RequestOperation::Bind(BindRequest {
                version: 3,
                dn: String::new(),
                authentication: BindAuthentication::Simple(vec![0xff; password_len]),
            });

            fixtures.push(test_wire::encode_request(128, request));
        }

        for packet in fixtures {
            for split in 0..packet.len() {
                let mut input: BytesMut = BytesMut::from(&packet[..split]);
                let mut codec: ServerCodec = ServerCodec::default();

                assert!(codec.decode(&mut input).unwrap().is_none());
                assert_eq!(&input[..], &packet[..split]);

                input.extend_from_slice(&packet[split..]);

                assert!(codec.decode(&mut input).unwrap().is_some());
                assert!(input.is_empty());
            }
        }
    }

    #[test]
    fn coalesced_frames_leave_the_following_message_untouched() {
        let mut input: BytesMut = BytesMut::from(SIMPLE_BIND);
        input.extend_from_slice(ROOT_DSE_SEARCH);

        let mut codec: ServerCodec = ServerCodec::default();

        let first: Request = codec.decode(&mut input).unwrap().unwrap();

        assert!(matches!(first.operation, RequestOperation::Bind(_)));
        assert_eq!(&input[..], ROOT_DSE_SEARCH);

        let second: Request = codec.decode(&mut input).unwrap().unwrap();

        assert!(matches!(second.operation, RequestOperation::Search(_)));
        assert!(input.is_empty());
    }

    #[test]
    fn definite_long_lengths_allow_one_to_four_octets() {
        for length_octet_count in 1..=4 {
            // BER permits leading zeros in a definite long-form length.
            let mut packet: Vec<u8> = vec![0x30, 0x80 | length_octet_count];
            packet.extend(vec![0; usize::from(length_octet_count) - 1]);
            packet.push(SIMPLE_BIND[1]);
            packet.extend_from_slice(&SIMPLE_BIND[2..]);

            decode_complete_packet(&packet);
        }
    }

    #[test]
    fn size_limits_are_checked_from_the_header_without_body_allocation() {
        let headers: [(&[u8], Option<DecodeErrorReason>); 6] = [
            (b"\x30\x80", Some(DecodeErrorReason::IndefiniteLength)),
            (b"\x30\xff", None), // Reserved length marker.
            (
                b"\x30\x85",
                Some(DecodeErrorReason::TooManyLengthOctets { count: 5, limit: 4 }),
            ),
            (b"\x3f\x00", None),                 // Extended tag number.
            (b"\x30\x84\xff\xff\xff\xff", None), // Maximum u32 length.
            (
                b"\x30\x82\x80\x00",
                Some(DecodeErrorReason::FrameTooLarge {
                    declared: 32772,
                    limit: 32768,
                }),
            ),
        ];

        for (header, reason) in headers {
            let diagnostic: DecodeError = assert_invalid_packet(header, "invalid frame header");
            if let Some(reason) = reason {
                assert_eq!(diagnostic, DecodeError::new(reason));
            }
        }

        // A 32764-byte body plus its four-byte header fits the 32 KiB limit.
        let mut incomplete: BytesMut = BytesMut::from(b"\x30\x82\x7f\xfc".as_slice());

        assert!(
            ServerCodec::default()
                .decode(&mut incomplete)
                .unwrap()
                .is_none()
        );

        // For this empty-DN Bind, all enclosing headers total 20 bytes.
        let bind: RequestOperation = RequestOperation::Bind(BindRequest {
            version: 3,
            dn: String::new(),
            authentication: BindAuthentication::Simple(vec![0xff; MAX_MESSAGE_SIZE - 20]),
        });
        let packet: Vec<u8> = test_wire::encode_request(1, bind);

        assert_eq!(packet.len(), MAX_MESSAGE_SIZE);
        decode_complete_packet(&packet);

        let oversized_bind: RequestOperation = RequestOperation::Bind(BindRequest {
            version: 3,
            dn: String::new(),
            authentication: BindAuthentication::Simple(vec![0xff; MAX_MESSAGE_SIZE - 19]),
        });
        let oversized: Vec<u8> = test_wire::encode_request(1, oversized_bind);

        // Only the header is supplied: rejection must not wait for the body.
        assert_invalid_packet(&oversized[..4], "frame one byte above limit");

        let oversized_buffer: Vec<u8> = vec![0; MAX_BUFFER_SIZE + 1];

        let diagnostic: DecodeError =
            assert_invalid_packet(&oversized_buffer, "buffer above limit");
        assert_eq!(
            diagnostic,
            DecodeError::new(DecodeErrorReason::BufferTooLarge {
                received: MAX_BUFFER_SIZE + 1,
                limit: MAX_BUFFER_SIZE,
            }),
        );
    }

    #[test]
    fn malformed_fields_scalars_and_nested_lengths_are_rejected() {
        let mut empty_message_id: Vec<u8> = SIMPLE_BIND.to_vec();
        empty_message_id[3] = 0;
        assert_invalid_packet(&empty_message_id, "empty message ID encoding");

        // Offsets refer to the fixed Simple Bind fixture above.
        let bind_mutations: [(usize, u8, DecodeErrorReason); 6] = [
            (6, 100, DecodeErrorReason::TruncatedElement),
            (
                10,
                0x24,
                DecodeErrorReason::UnexpectedTag {
                    expected: 0x04,
                    actual: 0x24,
                },
            ),
            (11, 100, DecodeErrorReason::TruncatedElement),
            (12, 0xff, DecodeErrorReason::InvalidUtf8),
            (
                17,
                0xa0,
                DecodeErrorReason::UnrecognizedAuthenticationTag { tag: 0xa0 },
            ),
            (18, 100, DecodeErrorReason::TruncatedElement),
        ];

        for (offset, replacement, reason) in bind_mutations {
            let mut packet: Vec<u8> = SIMPLE_BIND.to_vec();
            packet[offset] = replacement;

            let diagnostic: DecodeError = assert_invalid_packet(&packet, "Bind diagnostic");
            assert_eq!(
                diagnostic,
                DecodeError::new(reason).with_context(Some(1), Some(0x60)),
                "Bind offset={offset}",
            );
        }

        // Offsets refer to the fixed RootDSE search fixture above.
        let search_mutations: [(usize, u8, DecodeErrorReason); 5] = [
            (11, 4, DecodeErrorReason::InvalidSearchScope { value: 4 }),
            (
                14,
                4,
                DecodeErrorReason::InvalidAliasDereferencing { value: 4 },
            ),
            (22, 0, DecodeErrorReason::InvalidBoolean),
            (22, 2, DecodeErrorReason::InvalidBoolean),
            (23, 1, DecodeErrorReason::InvalidBoolean),
        ];

        for (offset, replacement, reason) in search_mutations {
            let mut packet: Vec<u8> = ROOT_DSE_SEARCH.to_vec();
            packet[offset] = replacement;

            let diagnostic: DecodeError = assert_invalid_packet(&packet, "Search diagnostic");
            assert_eq!(
                diagnostic,
                DecodeError::new(reason).with_context(Some(1), Some(0x63)),
                "Search offset={offset}",
            );
        }

        let mut trailing_fields: Vec<u8> = SIMPLE_BIND[7..].to_vec();
        trailing_fields.extend_from_slice(b"\x04\x00");

        let bind: Vec<u8> = test_wire::encode_element(0x60, &trailing_fields);
        let packet: Vec<u8> = test_wire::encode_message(1, &bind);

        assert_invalid_packet(&packet, "unexpected trailing Bind field");

        // An equality assertion cannot borrow bytes from the following selector.
        let packet: Vec<u8> = root_search_packet(b"\xa3\x03\x04\x04x");

        assert_invalid_packet(&packet, "assertion extends beyond filter container");

        let malformed_operations: [(&[u8], DecodeErrorReason); 4] = [
            (b"\x42\x01\x00", DecodeErrorReason::NonemptyUnbind),
            (
                b"\x50\x01\x00",
                DecodeErrorReason::InvalidAbandonTarget { value: 0 },
            ),
            (
                b"\x50\x00",
                DecodeErrorReason::InvalidIntegerLength { length: 0 },
            ),
            (
                b"\x61\x00",
                DecodeErrorReason::UnrecognizedOperationTag { tag: 0x61 },
            ),
        ];

        for (operation, reason) in malformed_operations {
            let packet: Vec<u8> = test_wire::encode_message(7, operation);
            let diagnostic: DecodeError = assert_invalid_packet(&packet, "operation diagnostic");
            assert_eq!(
                diagnostic,
                DecodeError::new(reason).with_context(Some(7), Some(operation[0])),
            );
        }

        let mut invalid_mechanism: Vec<u8> = LOGIN_WITHOUT_CREDENTIALS.to_vec();
        invalid_mechanism[16] = 255; // First octet of "LOGIN".

        assert_invalid_packet(&invalid_mechanism, "SASL mechanism is not UTF-8");
    }

    #[test]
    fn search_scopes_validate_the_complete_signed_integer() {
        let invalid_scopes: [(&str, &[u8]); 2] = [
            ("negative scope", b"\x0a\x01\xff"),
            ("256 must not become base scope", b"\x0a\x02\x01\x00"),
        ];

        for (case, encoded_scope) in invalid_scopes {
            // Replace only the scope field in the independent Search fixture.
            // The base occupies bytes 7..9; the remaining fields start at 12.
            let mut fields: Vec<u8> = ROOT_DSE_SEARCH[7..9].to_vec();
            fields.extend_from_slice(encoded_scope);
            fields.extend_from_slice(&ROOT_DSE_SEARCH[12..]);

            let operation: Vec<u8> = test_wire::encode_element(0x63, &fields);
            let packet: Vec<u8> = test_wire::encode_message(1, &operation);

            assert_invalid_packet(&packet, case);
        }
    }

    #[test]
    fn message_ids_cover_signed_integer_boundaries_without_truncation() {
        for message_id in [1, 127, 128, 255, 256, 32767, 32768, i32::MAX] {
            let packet: Vec<u8> = test_wire::encode_message(message_id, b"\x42\x00");

            let request: Request = decode_complete_packet(&packet);

            assert_eq!(request.message_id, message_id);
        }

        let invalid_ids: [(&str, &[u8]); 3] = [
            ("zero", b"\x30\x05\x02\x01\x00\x42\x00"),
            ("negative", b"\x30\x05\x02\x01\xff\x42\x00"),
            (
                "five-octet integer would truncate",
                b"\x30\x09\x02\x05\x01\x00\x00\x00\x01\x42\x00",
            ),
        ];

        for (case, packet) in invalid_ids {
            assert_invalid_packet(packet, case);
        }
    }

    #[test]
    fn filter_depth_node_counts_and_not_cardinality_are_bounded() {
        let mut nested_filter: Vec<u8> = test_wire::encode_element(0x87, b"objectClass");

        for _ in 1..MAX_FILTER_DEPTH {
            nested_filter = test_wire::encode_element(0xa2, &nested_filter);
        }

        decode_complete_packet(&root_search_packet(&nested_filter));

        let too_deep: Vec<u8> = test_wire::encode_element(0xa2, &nested_filter);

        let depth_error: DecodeError =
            assert_invalid_packet(&root_search_packet(&too_deep), "filter exceeds depth limit");

        assert_eq!(
            depth_error,
            DecodeError::new(DecodeErrorReason::FilterDepthLimit {
                limit: MAX_FILTER_DEPTH
            })
            .with_context(Some(1), Some(0x63))
        );

        // The AND node itself also counts toward the total node budget.
        let children: Vec<u8> = b"\x87\x01x".repeat(MAX_FILTER_NODES - 1);
        let wide_filter: Vec<u8> = test_wire::encode_element(0xa0, &children);

        decode_complete_packet(&root_search_packet(&wide_filter));

        let too_many_children: Vec<u8> = b"\x87\x01x".repeat(MAX_FILTER_NODES);
        let too_wide: Vec<u8> = test_wire::encode_element(0xa0, &too_many_children);

        let node_error: DecodeError =
            assert_invalid_packet(&root_search_packet(&too_wide), "filter exceeds node limit");

        assert_eq!(
            node_error,
            DecodeError::new(DecodeErrorReason::FilterNodeLimit {
                limit: MAX_FILTER_NODES
            })
            .with_context(Some(1), Some(0x63))
        );

        let invalid_sets: [(&str, &[u8]); 4] = [
            ("empty AND", b"\xa0\x00"),
            ("empty OR", b"\xa1\x00"),
            ("NOT without child", b"\xa2\x00"),
            ("NOT with two children", b"\xa2\x06\x87\x01x\x87\x01y"),
        ];

        for (case, filter) in invalid_sets {
            assert_invalid_packet(&root_search_packet(filter), case);
        }
    }

    #[test]
    fn control_boundaries_tags_and_booleans_are_validated() {
        let invalid_controls: [(&str, &[u8]); 4] = [
            (
                "invalid criticality BOOLEAN",
                b"\x30\x0a\x04\x051.2.3\x01\x01\x01",
            ),
            ("missing control OID", b"\x30\x00"),
            ("OID extends beyond control", b"\x30\x02\x04\x01"),
            (
                "truncated field after control",
                b"\x30\x07\x04\x051.2.3\x01",
            ),
        ];

        for (case, control) in invalid_controls {
            let mut envelope_content: Vec<u8> = SIMPLE_BIND[2..].to_vec();
            envelope_content.extend(test_wire::encode_element(0xa0, control));

            let packet: Vec<u8> = test_wire::encode_element(0x30, &envelope_content);

            assert_invalid_packet(&packet, case);
        }
    }

    #[test]
    fn replies_match_independent_ldap_wire_fixtures() {
        let anonymous_success: ResponseOperation = ResponseOperation::Bind {
            code: ResultCode::Success,
            challenge: None,
            diagnostic: None,
        };
        let login_challenge: ResponseOperation = ResponseOperation::Bind {
            code: ResultCode::SaslBindInProgress,
            challenge: Some(b"Username:".to_vec()),
            diagnostic: None,
        };
        let invalid_dn: ResponseOperation = ResponseOperation::Bind {
            code: ResultCode::InvalidDNSyntax,
            challenge: None,
            diagnostic: Some("invalid DN"),
        };
        let search_done: ResponseOperation = ResponseOperation::Result {
            kind: ResultKind::SearchDone,
            code: ResultCode::Success,
        };
        let root_entry: ResponseOperation = ResponseOperation::SearchEntry(vec![Attribute {
            name: "objectClass".to_owned(),
            values: vec![b"top".to_vec()],
        }]);

        let fixtures: [(ResponseOperation, &[u8]); 5] = [
            (anonymous_success, b"\x30\x0c\x02\x01\x01\x61\x07\x0a\x01\x00\x04\x00\x04\x00"),
            (login_challenge, b"\x30\x17\x02\x01\x01\x61\x12\x0a\x01\x0e\x04\x00\x04\x00\x87\x09Username:"),
            (
                invalid_dn,
                b"\x30\x16\x02\x01\x01\x61\x11\x0a\x01\x22\x04\x00\x04\x0ainvalid DN",
            ),
            (search_done, b"\x30\x0c\x02\x01\x01\x65\x07\x0a\x01\x00\x04\x00\x04\x00"),
            (root_entry, b"\x30\x1f\x02\x01\x01\x64\x1a\x04\x00\x30\x16\x30\x14\x04\x0bobjectClass\x31\x05\x04\x03top"),
        ];

        for (operation, expected_bytes) in fixtures {
            let encoded: BytesMut = encode_reply(1, operation);

            assert_eq!(&encoded[..], expected_bytes);
        }
    }

    #[test]
    fn reply_integer_and_length_boundaries_and_atomic_encoding() {
        for (message_id, challenge_len) in [
            (127, 0),
            (128, 127),
            (255, 128),
            (256, 255),
            (i32::MAX, 256),
        ] {
            let expected: Vec<u8> = vec![0xff; challenge_len];
            let operation: ResponseOperation = ResponseOperation::Bind {
                code: ResultCode::SaslBindInProgress,
                challenge: Some(expected.clone()),
                diagnostic: None,
            };
            let mut encoded: BytesMut = encode_reply(message_id, operation);
            let reply: Response = test_wire::decode_response(&mut encoded).unwrap();

            assert_eq!(reply.message_id, message_id);
            let ResponseOperation::Bind { challenge, .. } = reply.operation else {
                panic!("expected BindResponse");
            };

            assert_eq!(challenge, Some(expected));
            assert!(encoded.is_empty());
        }

        let oversized_reply: Response = Response {
            message_id: 1,
            operation: ResponseOperation::Bind {
                code: ResultCode::Success,
                challenge: Some(vec![0; MAX_MESSAGE_SIZE]),
                diagnostic: None,
            },
        };
        let mut output: BytesMut = BytesMut::from(b"already encoded".as_slice());

        assert!(
            ServerCodec::default()
                .encode(oversized_reply, &mut output)
                .is_err()
        );
        assert_eq!(&output[..], b"already encoded");
    }

    #[test]
    fn single_byte_mutations_of_valid_packets_never_panic() {
        for fixture in [SIMPLE_BIND, ROOT_DSE_SEARCH, LOGIN_WITHOUT_CREDENTIALS] {
            for offset in 0..fixture.len() {
                for replacement in 0..=255 {
                    let mut packet: BytesMut = BytesMut::from(fixture);
                    packet[offset] = replacement;

                    // Some mutations remain valid or incomplete; all must be safe.
                    let _ = ServerCodec::default().decode(&mut packet);
                }
            }
        }
    }
}
