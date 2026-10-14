use std::fmt;
use std::net::SocketAddr;

use bytes::Bytes;
use chrono::{DateTime, Local};

use super::ResultCode;
use super::error::DecodeError;
use super::protocol::BindRequest;

#[derive(Debug)]
pub(crate) struct LdapEvent {
    pub timestamp: DateTime<Local>,
    pub connection_id: u64,
    pub peer: SocketAddr,
    pub kind: LdapEventKind,
}

#[derive(Debug)]
pub(crate) enum LdapEventKind {
    Connected,
    Operation {
        message_id: i32,
        operation: OperationKind,
        result: Option<ResultCode>,
        diagnostic: Option<&'static str>,
    },
    BindReceived(BindEvent),
    RawMessage(RawMessage),
    Disconnected {
        reason: DisconnectReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperationKind {
    SimpleBind,
    PlainBind,
    LoginBind,
    UnsupportedSaslBind,
    UnknownBind,
    SicilyPackageDiscovery,
    SicilyNegotiate,
    SicilyResponse,
    Search,
    Unbind,
    Abandon,
    Modify,
    Add,
    Delete,
    ModifyDn,
    Compare,
    Extended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisconnectReason {
    Shutdown,
    PeerClosed,
    Unbind,
    DecodeFailed(DecodeError),
    UnexpectedOperation,
    ReadFailed,
    ReadTimeout,
    ResponseEncodingFailed,
    WriteFailed,
    WriteTimeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoginPhase {
    Username,
    Password,
}

#[derive(Debug)]
pub(crate) struct BindEvent {
    pub message_id: i32,
    pub request: BindRequest,
    pub login_phase: Option<LoginPhase>,
}

pub(crate) struct RawMessage {
    pub bytes: Bytes,
}

impl fmt::Debug for RawMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RawMessage")
            .field("length", &self.bytes.len())
            .finish_non_exhaustive()
    }
}
