use std::{fmt, str};

use crate::hexdump::HexDump;
use crate::ldap::event::{BindEvent, DisconnectReason, LoginPhase, OperationKind};
use crate::ldap::{
    BindAuthentication, LdapEvent, LdapEventKind, PlainCredentials, SaslMechanism,
    SicilyAuthentication,
};

pub(super) struct EventOutput<'a> {
    event: &'a LdapEvent,
}

impl<'a> EventOutput<'a> {
    pub(super) fn new(event: &'a LdapEvent) -> Self {
        Self { event }
    }
}

impl fmt::Display for EventOutput<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let event: &LdapEvent = self.event;

        write!(
            formatter,
            "[{}] #{} {} ",
            event.timestamp.format("%Y-%m-%d %H:%M:%S"),
            event.connection_id,
            event.peer
        )?;

        match &event.kind {
            LdapEventKind::Connected => writeln!(formatter, "connected"),
            LdapEventKind::Disconnected { reason } => {
                write!(formatter, "disconnected ({}", disconnect_label(*reason))?;

                if let DisconnectReason::DecodeFailed(diagnostic) = reason {
                    write!(formatter, ": {diagnostic}")?;
                }

                writeln!(formatter, ")")
            }
            LdapEventKind::Operation {
                message_id,
                operation,
                result,
                diagnostic,
            } => {
                write!(
                    formatter,
                    "message={message_id} {}",
                    operation_label(*operation)
                )?;

                if let Some(code) = result {
                    write!(formatter, " -> {code:?}")?;
                }

                if let Some(diagnostic) = diagnostic {
                    write!(formatter, " ({diagnostic})")?;
                }

                writeln!(formatter)
            }
            LdapEventKind::BindReceived(bind) => format_bind(formatter, bind),
            LdapEventKind::RawMessage(message) => {
                writeln!(
                    formatter,
                    "received BER message ({} bytes)",
                    message.bytes.len()
                )?;
                write!(formatter, "{}", HexDump::new(&message.bytes))
            }
        }
    }
}

fn operation_label(operation: OperationKind) -> &'static str {
    match operation {
        OperationKind::SimpleBind => "Simple Bind",
        OperationKind::PlainBind => "SASL PLAIN Bind",
        OperationKind::LoginBind => "SASL LOGIN Bind",
        OperationKind::UnsupportedSaslBind => "Unsupported SASL Bind",
        OperationKind::UnknownBind => "Unknown Bind",
        OperationKind::SicilyPackageDiscovery => "Sicily Package Discovery",
        OperationKind::SicilyNegotiate => "Sicily Negotiate",
        OperationKind::SicilyResponse => "Sicily Response",
        OperationKind::Search => "Search",
        OperationKind::Unbind => "Unbind",
        OperationKind::Abandon => "Abandon",
        OperationKind::Modify => "Modify",
        OperationKind::Add => "Add",
        OperationKind::Delete => "Delete",
        OperationKind::ModifyDn => "Modify DN",
        OperationKind::Compare => "Compare",
        OperationKind::Extended => "Extended Request",
    }
}

fn disconnect_label(reason: DisconnectReason) -> &'static str {
    match reason {
        DisconnectReason::Shutdown => "shutdown",
        DisconnectReason::PeerClosed => "peer closed",
        DisconnectReason::Unbind => "unbind",
        DisconnectReason::DecodeFailed(_) => "LDAP decode failed",
        DisconnectReason::UnexpectedOperation => "unexpected LDAP operation",
        DisconnectReason::ReadFailed => "read failed",
        DisconnectReason::ReadTimeout => "read timeout",
        DisconnectReason::ResponseEncodingFailed => "response encoding failed",
        DisconnectReason::WriteFailed => "write failed",
        DisconnectReason::WriteTimeout => "write timeout",
    }
}

fn format_bind(formatter: &mut fmt::Formatter<'_>, bind: &BindEvent) -> fmt::Result {
    write!(
        formatter,
        "message={} Bind received version={} DN={} ",
        bind.message_id,
        bind.request.version,
        EscapedBytes::new(bind.request.dn.as_bytes())
    )?;

    match &bind.request.authentication {
        BindAuthentication::Simple(password) => {
            writeln!(
                formatter,
                "method=Simple password={}",
                EscapedBytes::new(password)
            )
        }
        BindAuthentication::Sasl {
            mechanism,
            credentials,
        } => match mechanism {
            SaslMechanism::Plain => {
                formatter.write_str("method=SASL PLAIN")?;

                if credentials.is_empty() {
                    return writeln!(formatter, " credentials=<empty>");
                }

                match PlainCredentials::decode(credentials) {
                    Some(fields) => writeln!(
                        formatter,
                        " authzid={} authcid={} password={}",
                        EscapedBytes::new(fields.authorization_id.as_bytes()),
                        EscapedBytes::new(fields.authentication_id.as_bytes()),
                        EscapedBytes::new(fields.password)
                    ),
                    None => writeln!(
                        formatter,
                        " invalid PLAIN payload ({} bytes)",
                        credentials.len()
                    ),
                }
            }
            SaslMechanism::Login => {
                formatter.write_str("method=SASL LOGIN")?;

                match bind.login_phase {
                    Some(LoginPhase::Username) => {
                        writeln!(formatter, " username={}", EscapedBytes::new(credentials))
                    }
                    Some(LoginPhase::Password) => {
                        writeln!(formatter, " password={}", EscapedBytes::new(credentials))
                    }
                    None => writeln!(formatter, " credentials=<empty>"),
                }
            }
            SaslMechanism::Other(name) => {
                write!(
                    formatter,
                    "method=SASL {} unsupported",
                    EscapedBytes::new(name.as_bytes())
                )?;
                format_unsupported_payload(formatter, credentials)
            }
        },
        BindAuthentication::Sicily(authentication) => {
            let kind: &str = match authentication {
                SicilyAuthentication::PackageDiscovery(_) => "Package Discovery",
                SicilyAuthentication::Negotiate(_) => "Negotiate",
                SicilyAuthentication::Response(_) => "Response",
            };

            write!(formatter, "method=Sicily {kind} unsupported")?;
            format_unsupported_payload(formatter, authentication.payload())
        }
        BindAuthentication::Unknown { tag, credentials } => {
            write!(formatter, "method=Unknown (0x{tag:02x}) unsupported")?;
            format_unsupported_payload(formatter, credentials)
        }
    }
}

fn format_unsupported_payload(formatter: &mut fmt::Formatter<'_>, payload: &[u8]) -> fmt::Result {
    writeln!(formatter, " payload_length={}", payload.len())?;
    write!(formatter, "{}", HexDump::new(payload))
}

/// Quote received text without allowing terminal control sequences to execute.
struct EscapedBytes<'a> {
    bytes: &'a [u8],
}

impl<'a> EscapedBytes<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}

impl fmt::Display for EscapedBytes<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("\"")?;

        match str::from_utf8(self.bytes) {
            Ok(text) => {
                for character in text.chars() {
                    for escaped in character.escape_debug() {
                        write!(formatter, "{escaped}")?;
                    }
                }
            }
            Err(_) => {
                // Binary credentials use byte escapes, preserving every octet.
                for byte in self.bytes {
                    for escaped in byte.escape_ascii() {
                        write!(formatter, "{}", char::from(escaped))?;
                    }
                }
            }
        }

        formatter.write_str("\"")
    }
}
