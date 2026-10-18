use bytes::{Bytes, BytesMut};

const SEPARATOR: &[u8] = b" ";
const MESSAGE: &[u8] = b"Welcome";
const TERMINATOR: &[u8] = b"\r\n";

pub(super) struct FtpPacket {
    code: Bytes,
    message: Bytes,
}

impl FtpPacket {
    #[inline]
    pub(super) fn new<C, M>(code: C, message: M) -> Self
    where
        C: Into<Bytes>,
        M: Into<Bytes>,
    {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    #[inline]
    pub(super) fn welcome() -> Self {
        Self::new("220", MESSAGE)
    }

    #[inline]
    pub(super) fn syntax_error() -> Self {
        Self::new("501", "Syntax error in parameters")
    }

    #[inline]
    pub(super) fn bad_sequence() -> Self {
        Self::new("503", "Bad sequence of commands.")
    }

    #[inline]
    pub(super) fn password_required() -> Self {
        Self::new("331", "Password required.")
    }

    #[inline]
    pub(super) fn login_successful() -> Self {
        Self::new("230", "Login successful.")
    }

    // #[inline]
    // pub(super) fn login_incorrect() -> Self {
    //     Self::new("530", "Login incorrect")
    // }

    #[inline]
    pub(super) fn syst() -> Self {
        Self::new("215", "UNIX Type: L8")
    }

    #[inline]
    pub(super) fn noop() -> Self {
        Self::new("200", "No operation.")
    }

    #[inline]
    pub(super) fn quit() -> Self {
        Self::new("221", "Goodbye.")
    }

    #[inline]
    pub(super) fn command_not_implemented() -> Self {
        Self::new("502", "Command not implemented.")
    }

    pub(super) fn to_bytes(&self) -> Bytes {
        let mut packet = BytesMut::with_capacity(
            self.code.len() + SEPARATOR.len() + self.message.len() + TERMINATOR.len(),
        );

        packet.extend_from_slice(&self.code);
        packet.extend_from_slice(SEPARATOR);
        packet.extend_from_slice(&self.message);
        packet.extend_from_slice(TERMINATOR);

        packet.freeze()
    }
}
