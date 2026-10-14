//! LDAP session policy

use std::{io, mem, str};

use super::event::{BindEvent, LoginPhase, OperationKind};
use super::protocol::{
    Attribute, BindAuthentication, BindRequest, Filter, PlainCredentials, Request,
    RequestOperation, Response, ResponseOperation, ResultCode, ResultKind, SaslMechanism,
    SearchRequest, SearchScope, SicilyAuthentication, UnsupportedOperation,
};

#[derive(Default)]
enum SaslState {
    #[default]
    Idle,
    AwaitingPlainCredentials,
    AwaitingLoginUsername,
    AwaitingLoginPassword,
}

impl SaslState {
    #[inline]
    fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    #[inline]
    fn is_awaiting_plain_credentials(&self) -> bool {
        matches!(self, Self::AwaitingPlainCredentials)
    }
}

struct BindOutcome {
    code: ResultCode,
    challenge: Option<Vec<u8>>,
    diagnostic: Option<&'static str>,
}

impl BindOutcome {
    fn success() -> Self {
        Self {
            code: ResultCode::Success,
            challenge: None,
            diagnostic: None,
        }
    }

    fn challenge(challenge: Vec<u8>) -> Self {
        Self {
            code: ResultCode::SaslBindInProgress,
            challenge: Some(challenge),
            diagnostic: None,
        }
    }

    fn rejected(code: ResultCode) -> Self {
        Self {
            code,
            challenge: None,
            diagnostic: None,
        }
    }

    fn rejected_with_diagnostic(code: ResultCode, diagnostic: &'static str) -> Self {
        Self {
            code,
            challenge: None,
            diagnostic: Some(diagnostic),
        }
    }
}

pub(super) struct HandledOperation {
    pub operation: OperationKind,
    pub result: Option<ResultCode>,
    pub diagnostic: Option<&'static str>,
    pub responses: Vec<Response>,
    pub bind: Option<BindEvent>,
    pub close_connection: bool,
}

impl HandledOperation {
    fn reject(
        &mut self,
        message_id: i32,
        request: &RequestOperation,
        code: ResultCode,
        diagnostic: Option<&'static str>,
    ) -> io::Result<()> {
        let response: ResponseOperation = match request {
            RequestOperation::Bind(_) => ResponseOperation::Bind {
                code,
                challenge: None,
                diagnostic,
            },
            RequestOperation::Search(_) => ResponseOperation::Result {
                kind: ResultKind::SearchDone,
                code,
            },
            RequestOperation::Unsupported(operation) => ResponseOperation::Result {
                kind: ResultKind::Rejected(*operation),
                code,
            },
            RequestOperation::Unbind | RequestOperation::Abandon => {
                return Err(io::Error::other("operation has no response"));
            }
        };

        self.result = Some(code);
        self.diagnostic = diagnostic;
        self.responses.push(Response {
            message_id,
            operation: response,
        });

        Ok(())
    }
}

#[derive(Default)]
pub(super) struct Session {
    sasl: SaslState,
}

impl Session {
    pub(super) fn handle(&mut self, request: Request) -> io::Result<HandledOperation> {
        let Request {
            message_id,
            operation,
            critical_control,
        } = request;
        let mut handled: HandledOperation = HandledOperation {
            operation: operation_kind(&operation),
            result: None,
            diagnostic: None,
            responses: Vec::new(),
            bind: None,
            close_connection: false,
        };

        match operation {
            RequestOperation::Unbind => {
                self.sasl = SaslState::Idle;
                handled.close_connection = true;
            }
            RequestOperation::Abandon => {}
            RequestOperation::Bind(bind_request) => {
                let login_phase: Option<LoginPhase> = self.login_phase(&bind_request);
                let outcome: BindOutcome = if critical_control {
                    self.sasl = SaslState::Idle;
                    BindOutcome::rejected_with_diagnostic(
                        ResultCode::UnavailableCriticalExtension,
                        "critical extension is not recognized",
                    )
                } else {
                    self.handle_bind(&bind_request)
                };

                handled.result = Some(outcome.code);
                handled.diagnostic = outcome.diagnostic;
                handled.responses.push(Response {
                    message_id,
                    operation: ResponseOperation::Bind {
                        code: outcome.code,
                        challenge: outcome.challenge,
                        diagnostic: outcome.diagnostic,
                    },
                });
                handled.bind = Some(BindEvent {
                    message_id,
                    request: bind_request,
                    login_phase,
                });
            }
            operation if critical_control => {
                handled.reject(
                    message_id,
                    &operation,
                    ResultCode::UnavailableCriticalExtension,
                    Some("critical extension is not recognized"),
                )?;
            }
            operation if !self.sasl.is_idle() => {
                handled.reject(message_id, &operation, ResultCode::OperationsError, None)?;
            }
            RequestOperation::Search(search_request) => {
                let SearchOutcome { code, entry } = search_root_dse(&search_request);
                handled.result = Some(code);

                if let Some(attributes) = entry {
                    handled.responses.push(Response {
                        message_id,
                        operation: ResponseOperation::SearchEntry(attributes),
                    });
                }

                handled.responses.push(Response {
                    message_id,
                    operation: ResponseOperation::Result {
                        kind: ResultKind::SearchDone,
                        code,
                    },
                });
            }
            operation @ RequestOperation::Unsupported(_) => {
                handled.reject(message_id, &operation, ResultCode::UnwillingToPerform, None)?;
            }
        }

        Ok(handled)
    }

    fn login_phase(&self, request: &BindRequest) -> Option<LoginPhase> {
        let BindAuthentication::Sasl {
            mechanism: SaslMechanism::Login,
            credentials,
        } = &request.authentication
        else {
            return None;
        };

        match self.sasl {
            SaslState::AwaitingLoginPassword => Some(LoginPhase::Password),
            SaslState::AwaitingLoginUsername => Some(LoginPhase::Username),
            SaslState::Idle | SaslState::AwaitingPlainCredentials => {
                if credentials.is_empty() {
                    None
                } else {
                    Some(LoginPhase::Username)
                }
            }
        }
    }

    fn handle_bind(&mut self, request: &BindRequest) -> BindOutcome {
        let previous_state: SaslState = mem::take(&mut self.sasl);

        if !dn_is_plausible(&request.dn) {
            return BindOutcome::rejected_with_diagnostic(
                ResultCode::InvalidDNSyntax,
                "invalid DN",
            );
        }

        if request.version != 2 && request.version != 3 {
            return BindOutcome::rejected_with_diagnostic(
                ResultCode::ProtocolError,
                "requested protocol version not supported",
            );
        }

        if request.version == 2 {
            return BindOutcome::rejected_with_diagnostic(
                ResultCode::ProtocolError,
                "historical protocol version requested, use LDAPv3 instead",
            );
        }

        match &request.authentication {
            BindAuthentication::Simple(_) => BindOutcome::success(),
            BindAuthentication::Sicily(_) => BindOutcome::rejected_with_diagnostic(
                ResultCode::AuthMethodNotSupported,
                "unknown authentication method",
            ),
            BindAuthentication::Sasl {
                mechanism,
                credentials,
            } => match mechanism {
                SaslMechanism::Plain => {
                    let awaiting_credentials: bool = previous_state.is_awaiting_plain_credentials();
                    self.handle_plain_bind(credentials, awaiting_credentials)
                }
                SaslMechanism::Login => self.handle_login_bind(credentials, previous_state),
                SaslMechanism::Other(_) => BindOutcome::rejected_with_diagnostic(
                    ResultCode::AuthMethodNotSupported,
                    "unknown authentication method",
                ),
            },
            BindAuthentication::Unknown { .. } => BindOutcome::rejected_with_diagnostic(
                ResultCode::AuthMethodNotSupported,
                "unknown authentication method",
            ),
        }
    }

    fn handle_plain_bind(&mut self, credentials: &[u8], awaiting_credentials: bool) -> BindOutcome {
        if credentials.is_empty() && !awaiting_credentials {
            self.sasl = SaslState::AwaitingPlainCredentials;
            return BindOutcome::challenge(Vec::new());
        }

        match PlainCredentials::decode(credentials) {
            Some(_) => BindOutcome::success(),
            None => BindOutcome::rejected(ResultCode::ProtocolError),
        }
    }

    fn handle_login_bind(&mut self, credentials: &[u8], previous_state: SaslState) -> BindOutcome {
        match previous_state {
            SaslState::AwaitingLoginPassword => BindOutcome::success(),
            SaslState::AwaitingLoginUsername => self.accept_login_username(credentials),
            SaslState::Idle | SaslState::AwaitingPlainCredentials => {
                if credentials.is_empty() {
                    self.sasl = SaslState::AwaitingLoginUsername;
                    return BindOutcome::challenge(b"Username:".to_vec());
                }

                self.accept_login_username(credentials)
            }
        }
    }

    fn accept_login_username(&mut self, credentials: &[u8]) -> BindOutcome {
        match str::from_utf8(credentials) {
            Ok(username) if !username.is_empty() && !username.contains('\0') => {}
            _ => return BindOutcome::rejected(ResultCode::ProtocolError),
        }

        self.sasl = SaslState::AwaitingLoginPassword;

        BindOutcome::challenge(b"Password:".to_vec())
    }
}

fn operation_kind(request: &RequestOperation) -> OperationKind {
    match request {
        RequestOperation::Bind(bind_request) => match &bind_request.authentication {
            BindAuthentication::Simple(_) => OperationKind::SimpleBind,
            BindAuthentication::Sicily(authentication) => match authentication {
                SicilyAuthentication::PackageDiscovery(_) => OperationKind::SicilyPackageDiscovery,
                SicilyAuthentication::Negotiate(_) => OperationKind::SicilyNegotiate,
                SicilyAuthentication::Response(_) => OperationKind::SicilyResponse,
            },
            BindAuthentication::Sasl { mechanism, .. } => match mechanism {
                SaslMechanism::Plain => OperationKind::PlainBind,
                SaslMechanism::Login => OperationKind::LoginBind,
                SaslMechanism::Other(_) => OperationKind::UnsupportedSaslBind,
            },
            BindAuthentication::Unknown { .. } => OperationKind::UnknownBind,
        },
        RequestOperation::Search(_) => OperationKind::Search,
        RequestOperation::Unbind => OperationKind::Unbind,
        RequestOperation::Abandon => OperationKind::Abandon,
        RequestOperation::Unsupported(operation) => match operation {
            UnsupportedOperation::Modify => OperationKind::Modify,
            UnsupportedOperation::Add => OperationKind::Add,
            UnsupportedOperation::Delete => OperationKind::Delete,
            UnsupportedOperation::ModifyDn => OperationKind::ModifyDn,
            UnsupportedOperation::Compare => OperationKind::Compare,
            UnsupportedOperation::Extended => OperationKind::Extended,
        },
    }
}

/// A minimal RFC 4514 plausibility check for the Bind name, mirroring slapd's
/// DN-first rejection. Every unescaped-comma-separated RDN must contain an
/// unescaped '=' separating a non-empty attribute type and value. Bind names
/// such as "NTLM" or "DOMAIN\user" fail, which is what makes Windows-style
/// clients downgrade to a cleartext simple Bind.
fn dn_is_plausible(dn: &str) -> bool {
    if dn.is_empty() {
        return true;
    }

    let mut type_len: usize = 0;
    let mut value_len: usize = 0;
    let mut has_separator: bool = false;
    let mut escaped: bool = false;

    for byte in dn.bytes() {
        if escaped {
            // An escaped octet belongs to the current side of the separator.
            escaped = false;
            value_len += usize::from(has_separator);
        } else {
            match byte {
                b'\\' => escaped = true,
                b'=' if !has_separator => {
                    if type_len == 0 {
                        return false;
                    }

                    has_separator = true;
                }
                b',' => {
                    if !has_separator || value_len == 0 {
                        return false;
                    }

                    type_len = 0;
                    value_len = 0;
                    has_separator = false;
                }
                _ => {
                    if has_separator {
                        value_len += 1;
                    } else {
                        type_len += 1;
                    }
                }
            }
        }
    }

    !escaped && has_separator && value_len > 0
}

struct SearchOutcome {
    code: ResultCode,
    entry: Option<Vec<Attribute>>,
}

impl SearchOutcome {
    fn without_entry(code: ResultCode) -> Self {
        Self { code, entry: None }
    }
}

fn search_root_dse(request: &SearchRequest) -> SearchOutcome {
    if let Some(code) = validate_root_search(request) {
        return SearchOutcome::without_entry(code);
    }

    let mut attributes: Vec<Attribute> = root_attributes();

    match evaluate_filter(&request.filter, &attributes) {
        FilterEvaluation::Unsupported => {
            return SearchOutcome::without_entry(ResultCode::UnwillingToPerform);
        }
        FilterEvaluation::NoMatch => return SearchOutcome::without_entry(ResultCode::Success),
        FilterEvaluation::Match => {}
    }

    attributes.retain(|attribute| attribute_is_selected(attribute, &request.attributes));

    if request.types_only {
        for attribute in &mut attributes {
            attribute.values.clear();
        }
    }

    SearchOutcome {
        code: ResultCode::Success,
        entry: Some(attributes),
    }
}

fn validate_root_search(request: &SearchRequest) -> Option<ResultCode> {
    if !request.base.is_empty() {
        return Some(ResultCode::NoSuchObject);
    }

    if request.scope != SearchScope::Base {
        return Some(ResultCode::UnwillingToPerform);
    }

    if request.size_limit < 0 || request.time_limit < 0 {
        return Some(ResultCode::ProtocolError);
    }

    None
}

fn root_attributes() -> Vec<Attribute> {
    let descriptions: [(&str, &[&str]); 5] = [
        ("objectClass", &["top"]),
        ("supportedLDAPVersion", &["3"]),
        ("supportedSASLMechanisms", &["PLAIN", "LOGIN"]),
        ("vendorName", &["Redknife"]),
        ("vendorVersion", &[env!("CARGO_PKG_VERSION")]),
    ];

    let mut attributes: Vec<Attribute> = Vec::new();

    for (name, strings) in descriptions {
        let values: Vec<Vec<u8>> = strings
            .iter()
            .map(|value| value.as_bytes().to_vec())
            .collect();

        attributes.push(Attribute {
            name: name.to_owned(),
            values,
        });
    }

    attributes
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterEvaluation {
    Match,
    NoMatch,
    Unsupported,
}

fn evaluate_filter(filter: &Filter, attributes: &[Attribute]) -> FilterEvaluation {
    match filter {
        Filter::Present(name) => {
            for attribute in attributes {
                if attribute.name.eq_ignore_ascii_case(name) {
                    return FilterEvaluation::Match;
                }
            }

            FilterEvaluation::NoMatch
        }
        Filter::Equality(name, expected) => {
            for attribute in attributes {
                if !attribute.name.eq_ignore_ascii_case(name) {
                    continue;
                }

                for value in &attribute.values {
                    if value.eq_ignore_ascii_case(expected) {
                        return FilterEvaluation::Match;
                    }
                }
            }

            FilterEvaluation::NoMatch
        }
        Filter::And(children) => {
            let mut all_match: bool = true;

            for child in children {
                match evaluate_filter(child, attributes) {
                    FilterEvaluation::Match => {}
                    FilterEvaluation::NoMatch => all_match = false,
                    FilterEvaluation::Unsupported => return FilterEvaluation::Unsupported,
                }
            }

            if all_match {
                FilterEvaluation::Match
            } else {
                FilterEvaluation::NoMatch
            }
        }
        Filter::Or(children) => {
            let mut any_match: bool = false;

            for child in children {
                match evaluate_filter(child, attributes) {
                    FilterEvaluation::Match => any_match = true,
                    FilterEvaluation::NoMatch => {}
                    FilterEvaluation::Unsupported => return FilterEvaluation::Unsupported,
                }
            }

            if any_match {
                FilterEvaluation::Match
            } else {
                FilterEvaluation::NoMatch
            }
        }
        Filter::Not(child) => match evaluate_filter(child, attributes) {
            FilterEvaluation::Match => FilterEvaluation::NoMatch,
            FilterEvaluation::NoMatch => FilterEvaluation::Match,
            FilterEvaluation::Unsupported => FilterEvaluation::Unsupported,
        },
        Filter::Unsupported => FilterEvaluation::Unsupported,
    }
}

fn attribute_is_selected(attribute: &Attribute, selectors: &[String]) -> bool {
    let operational: bool = attribute.name != "objectClass";

    for selector in selectors {
        match selector.as_str() {
            "*" if !operational => return true,
            "+" if operational => return true,
            name if name.eq_ignore_ascii_case(&attribute.name) => return true,
            _ => {}
        }
    }

    selectors.is_empty() && !operational
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sicily_rejections_follow_the_bind_name_plausibility() {
        let cases: [(&str, ResultCode, &str); 2] = [
            (
                "cn=alice",
                ResultCode::AuthMethodNotSupported,
                "unknown authentication method",
            ),
            ("NTLM", ResultCode::InvalidDNSyntax, "invalid DN"),
        ];
        let payload: &[u8] = b"\xff\x00\x1b";

        for (dn, expected, diagnostic) in cases {
            let mut session: Session = Session::default();
            let request: Request = bind_with(
                3,
                dn,
                BindAuthentication::Sicily(SicilyAuthentication::Negotiate(payload.to_vec())),
            );
            let handled: HandledOperation = session.handle(request).unwrap();

            assert_eq!(handled.result, Some(expected));
            assert!(!handled.close_connection);
            assert_diagnostic(&handled, diagnostic);

            let bind: BindEvent = handled.bind.unwrap();
            let BindAuthentication::Sicily(authentication) = bind.request.authentication else {
                panic!("expected Sicily authentication");
            };

            assert_eq!(bind.request.dn, dn);
            assert_eq!(authentication.payload(), payload);
        }
    }

    #[test]
    fn dn_plausibility_covers_slapd_first_rejections() {
        let plausible: [&str; 5] = [
            "",
            "cn=alice",
            "cn=alice,dc=example,dc=com",
            "cn=a\\,b,dc=example",
            "cn=alice+sn=smith",
        ];

        for dn in plausible {
            assert!(dn_is_plausible(dn), "{dn:?} must be plausible");
        }

        let implausible: [&str; 7] = [
            "NTLM",
            "za.tryhackme.com\\svcLDAP",
            "alice",
            "=value",
            "cn=",
            "cn=alice,",
            "cn=al,ice",
        ];

        for dn in implausible {
            assert!(!dn_is_plausible(dn), "{dn:?} must not be plausible");
        }
    }

    #[test]
    fn bind_evaluation_order_mirrors_slapd_dn_version_then_method() {
        // DN syntax first: even an LDAPv2 Sicily Bind with name "NTLM" is
        // answered with invalidDNSyntax, not a protocol-version reply.
        let sicily: Request = bind_with(
            2,
            "NTLM",
            BindAuthentication::Sicily(SicilyAuthentication::Negotiate(b"NTLMSSP\0".to_vec())),
        );
        let handled: HandledOperation = Session::default().handle(sicily).unwrap();

        assert_eq!(handled.result, Some(ResultCode::InvalidDNSyntax));
        assert_diagnostic(&handled, "invalid DN");
        assert!(handled.bind.is_some());

        // Protocol version second: plausible DNs outside 2..=3 are rejected
        // with slapd's version diagnostic, and v2 Binds with the historical
        // version text, while their credentials are still captured.
        for (version, diagnostic) in [
            (4, "requested protocol version not supported"),
            (
                2,
                "historical protocol version requested, use LDAPv3 instead",
            ),
        ] {
            let request: Request = bind_with(
                version,
                "cn=alice,dc=example,dc=com",
                simple_auth(b"secret"),
            );
            let handled: HandledOperation = Session::default().handle(request).unwrap();

            assert_eq!(handled.result, Some(ResultCode::ProtocolError));
            assert_diagnostic(&handled, diagnostic);

            let bind: BindEvent = handled.bind.unwrap();

            assert_simple_payload(&bind, b"secret");
        }

        // Method last: a v3 Bind with an unknown primitive authentication
        // choice receives slapd's unknown-method reply.
        let request: Request = Request {
            message_id: 1,
            operation: RequestOperation::Bind(BindRequest {
                version: 3,
                dn: String::new(),
                authentication: BindAuthentication::Unknown {
                    tag: 0x8c,
                    credentials: b"opaque".to_vec(),
                },
            }),
            critical_control: false,
        };
        let handled: HandledOperation = Session::default().handle(request).unwrap();

        assert_eq!(handled.result, Some(ResultCode::AuthMethodNotSupported));
        assert_diagnostic(&handled, "unknown authentication method");

        let bind: BindEvent = handled.bind.unwrap();
        let BindAuthentication::Unknown { tag, credentials } = bind.request.authentication else {
            panic!("expected Unknown authentication");
        };

        assert_eq!(tag, 0x8c);
        assert_eq!(credentials, b"opaque");
    }

    fn bind_with(version: i32, dn: &str, authentication: BindAuthentication) -> Request {
        Request {
            message_id: 1,
            operation: RequestOperation::Bind(BindRequest {
                version,
                dn: dn.to_owned(),
                authentication,
            }),
            critical_control: false,
        }
    }

    fn simple_auth(password: &[u8]) -> BindAuthentication {
        BindAuthentication::Simple(password.to_vec())
    }

    fn assert_simple_payload(handled_bind: &BindEvent, expected: &[u8]) {
        let BindAuthentication::Simple(payload) = &handled_bind.request.authentication else {
            panic!("expected Simple Bind");
        };

        assert_eq!(payload, expected);
    }

    fn assert_diagnostic(handled: &HandledOperation, expected: &'static str) {
        let ResponseOperation::Bind { diagnostic, .. } = &handled.responses[0].operation else {
            panic!("expected BindResponse");
        };

        assert_eq!(*diagnostic, Some(expected));
        assert_eq!(handled.diagnostic, Some(expected));
    }

    fn bind_request(message_id: i32, mechanism: SaslMechanism, credentials: &[u8]) -> Request {
        Request {
            message_id,
            operation: RequestOperation::Bind(BindRequest {
                version: 3,
                dn: String::new(),
                authentication: BindAuthentication::Sasl {
                    mechanism,
                    credentials: credentials.to_vec(),
                },
            }),
            critical_control: false,
        }
    }

    fn assert_challenge(handled: &HandledOperation, expected: &[u8]) {
        assert_eq!(handled.result, Some(ResultCode::SaslBindInProgress));
        assert!(handled.bind.is_some());
        assert_eq!(handled.responses.len(), 1);

        let ResponseOperation::Bind { challenge, .. } = &handled.responses[0].operation else {
            panic!("expected BindResponse");
        };

        assert_eq!(challenge.as_deref(), Some(expected));
    }

    #[test]
    fn failed_login_username_resets_the_exchange_while_preserving_the_bind() {
        let mut session: Session = Session::default();

        let initial: HandledOperation = session
            .handle(bind_request(1, SaslMechanism::Login, b""))
            .unwrap();

        assert_challenge(&initial, b"Username:");

        let malformed: HandledOperation = session
            .handle(bind_request(2, SaslMechanism::Login, b"bad\0name"))
            .unwrap();

        assert_eq!(malformed.result, Some(ResultCode::ProtocolError));
        assert!(malformed.bind.is_some());

        let restarted: HandledOperation = session
            .handle(bind_request(3, SaslMechanism::Login, b""))
            .unwrap();

        assert_challenge(&restarted, b"Username:");
    }

    #[test]
    fn plain_rejects_invalid_continuations_and_resets() {
        for continuation in [b"".as_slice(), b"invalid-secret"] {
            let mut session: Session = Session::default();

            let initial: HandledOperation = session
                .handle(bind_request(1, SaslMechanism::Plain, b""))
                .unwrap();

            assert_challenge(&initial, b"");

            let malformed: HandledOperation = session
                .handle(bind_request(2, SaslMechanism::Plain, continuation))
                .unwrap();

            assert_eq!(malformed.result, Some(ResultCode::ProtocolError));
            assert!(malformed.bind.is_some());

            let restarted: HandledOperation = session
                .handle(bind_request(3, SaslMechanism::Plain, b""))
                .unwrap();

            assert_challenge(&restarted, b"");
        }
    }

    #[test]
    fn a_critical_control_rejects_bind_and_clears_its_pending_sasl_state() {
        let mut session: Session = Session::default();

        let initial: HandledOperation = session
            .handle(bind_request(1, SaslMechanism::Login, b"alice"))
            .unwrap();

        assert_challenge(&initial, b"Password:");

        let mut request: Request = bind_request(2, SaslMechanism::Login, b"password");
        request.critical_control = true;

        let rejected: HandledOperation = session.handle(request).unwrap();

        assert_eq!(
            rejected.result,
            Some(ResultCode::UnavailableCriticalExtension)
        );
        assert!(rejected.bind.is_some());

        let restarted: HandledOperation = session
            .handle(bind_request(3, SaslMechanism::Login, b""))
            .unwrap();

        assert_challenge(&restarted, b"Username:");
    }

    #[test]
    fn an_unsupported_sasl_mechanism_rejects_bind_and_resets_pending_exchanges() {
        let initial_exchanges: [(SaslMechanism, &[u8], &[u8]); 3] = [
            (SaslMechanism::Plain, b"", b""),
            (SaslMechanism::Login, b"", b"Username:"),
            (SaslMechanism::Login, b"alice", b"Password:"),
        ];

        for (mechanism, credentials, expected_challenge) in initial_exchanges {
            let mut session: Session = Session::default();

            let initial: HandledOperation = session
                .handle(bind_request(1, mechanism, credentials))
                .unwrap();

            assert_challenge(&initial, expected_challenge);

            let request: Request = bind_request(
                2,
                SaslMechanism::Other("GSSAPI".to_owned()),
                b"opaque-token",
            );

            let rejected: HandledOperation = session.handle(request).unwrap();

            assert_eq!(rejected.operation, OperationKind::UnsupportedSaslBind);
            assert_eq!(rejected.result, Some(ResultCode::AuthMethodNotSupported));
            assert!(rejected.bind.is_some());

            let restarted: HandledOperation = session
                .handle(bind_request(3, SaslMechanism::Login, b""))
                .unwrap();

            assert_challenge(&restarted, b"Username:");
        }
    }

    #[test]
    fn boolean_filters_do_not_hide_unsupported_children_after_a_decisive_result() {
        let cases: [Filter; 3] = [
            Filter::And(vec![
                Filter::Present("missing".to_owned()),
                Filter::Unsupported,
            ]),
            Filter::Or(vec![
                Filter::Present("objectClass".to_owned()),
                Filter::Unsupported,
            ]),
            Filter::Not(Box::new(Filter::Unsupported)),
        ];

        for filter in cases {
            let request: SearchRequest = root_search_request(&[], filter);

            let outcome: SearchOutcome = search_root_dse(&request);

            assert_eq!(outcome.code, ResultCode::UnwillingToPerform);
            assert!(outcome.entry.is_none());
        }
    }

    fn root_search_request(selectors: &[&str], filter: Filter) -> SearchRequest {
        SearchRequest {
            base: String::new(),
            scope: SearchScope::Base,
            size_limit: 0,
            time_limit: 0,
            types_only: false,
            filter,
            attributes: selectors.iter().map(|name| name.to_string()).collect(),
        }
    }

    #[test]
    fn root_dse_attribute_selection_and_types_only() {
        let cases: Vec<(Vec<&str>, Vec<&str>)> = vec![
            (
                vec!["supportedLDAPVersion", "supportedSASLMechanisms"],
                vec!["supportedLDAPVersion", "supportedSASLMechanisms"],
            ),
            (
                vec!["+"],
                vec![
                    "supportedLDAPVersion",
                    "supportedSASLMechanisms",
                    "vendorName",
                    "vendorVersion",
                ],
            ),
            (vec!["*"], vec!["objectClass"]),
            (vec![], vec!["objectClass"]),
            (vec!["1.1"], vec![]),
            (
                vec!["1.1", "SUPPORTEDLDAPVERSION"],
                vec!["supportedLDAPVersion"],
            ),
            (
                vec!["*", "+"],
                vec![
                    "objectClass",
                    "supportedLDAPVersion",
                    "supportedSASLMechanisms",
                    "vendorName",
                    "vendorVersion",
                ],
            ),
        ];

        for (selectors, mut expected_names) in cases {
            let request: SearchRequest =
                root_search_request(&selectors, Filter::Present("objectClass".to_owned()));
            let outcome: SearchOutcome = search_root_dse(&request);

            assert_eq!(outcome.code, ResultCode::Success);
            let attributes: Vec<Attribute> = outcome.entry.unwrap();
            let mut names: Vec<&str> = attributes
                .iter()
                .map(|attribute| attribute.name.as_str())
                .collect();
            names.sort_unstable();
            expected_names.sort_unstable();
            assert_eq!(names, expected_names);

            for attribute in attributes {
                match attribute.name.as_str() {
                    "supportedLDAPVersion" => assert_eq!(attribute.values, vec![b"3".to_vec()]),
                    "supportedSASLMechanisms" => {
                        assert_eq!(attribute.values, vec![b"PLAIN".to_vec(), b"LOGIN".to_vec()])
                    }
                    _ => {}
                }
            }
        }

        let mut request: SearchRequest =
            root_search_request(&["*", "+"], Filter::Present("objectClass".to_owned()));
        request.types_only = true;
        let outcome: SearchOutcome = search_root_dse(&request);

        assert_eq!(outcome.code, ResultCode::Success);
        let attributes: Vec<Attribute> = outcome.entry.unwrap();
        assert_eq!(attributes.len(), 5);
        assert!(
            attributes
                .iter()
                .all(|attribute| attribute.values.is_empty())
        );
    }

    #[test]
    fn discovery_filters_and_non_root_searches() {
        let filter: Filter = Filter::And(vec![
            Filter::Or(vec![
                Filter::Equality("supportedLDAPVersion".to_owned(), b"3".to_vec()),
                Filter::Present("missing".to_owned()),
            ]),
            Filter::Not(Box::new(Filter::Present("supportedExtension".to_owned()))),
        ]);
        let request: SearchRequest = root_search_request(&["+"], filter);
        let outcome: SearchOutcome = search_root_dse(&request);

        assert_eq!(outcome.code, ResultCode::Success);
        assert!(outcome.entry.is_some());

        let request: SearchRequest =
            root_search_request(&["+"], Filter::Present("missing".to_owned()));
        let outcome: SearchOutcome = search_root_dse(&request);

        assert_eq!(outcome.code, ResultCode::Success);
        assert!(outcome.entry.is_none());

        let mut request: SearchRequest =
            root_search_request(&[], Filter::Present("objectClass".to_owned()));
        request.base = "dc=example,dc=com".to_owned();
        let outcome: SearchOutcome = search_root_dse(&request);

        assert_eq!(outcome.code, ResultCode::NoSuchObject);
        assert!(outcome.entry.is_none());

        let request: SearchRequest = root_search_request(&[], Filter::Unsupported);
        let outcome: SearchOutcome = search_root_dse(&request);

        assert_eq!(outcome.code, ResultCode::UnwillingToPerform);
        assert!(outcome.entry.is_none());
    }
}
