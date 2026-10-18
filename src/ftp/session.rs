pub(super) enum FtpSessionLogin {
    LoggedOut,
    PendingPassword {
        username: String,
    },
    LoggedIn {
        username: String,
        // password: String,
    },
}

impl FtpSessionLogin {
    #[inline]
    pub(super) fn username(&self) -> Option<&String> {
        match &self {
            Self::PendingPassword { username } | Self::LoggedIn { username, .. } => Some(username),
            _ => None,
        }
    }

    // #[inline]
    // pub(super) fn password(&self) -> Option<&String> {
    //     match &self {
    //         Self::LoggedIn { password, .. } => Some(password),
    //         _ => None,
    //     }
    // }
    //
    // #[inline]
    // pub(super) fn logout(&mut self) {
    //     *self = Self::LoggedOut;
    // }
    //
    // #[inline]
    // pub(super) fn is_logged_in(&self) -> bool {
    //     matches!(self, Self::LoggedIn { .. })
    // }
}

pub(super) struct FtpSession {
    pub(super) login: FtpSessionLogin,
}

impl FtpSession {
    pub(super) fn new() -> Self {
        Self {
            login: FtpSessionLogin::LoggedOut,
        }
    }
}
