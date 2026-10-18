use std::io;

#[derive(Debug, Clone)]
pub(super) enum FtpResponse {
    User(String),
    Pass(String),
    Syst,
    Noop,
    Quit,
}

impl FtpResponse {
    pub(super) fn parse(mut bytes: Vec<u8>) -> io::Result<Self> {
        // Removes CRLF
        while matches!(bytes.last(), Some(b'\r' | b'\n')) {
            bytes.pop();
        }

        if bytes.starts_with(b"USER ") {
            let user: String = decode_latin1(&bytes[5..]).trim().to_string();
            return Ok(FtpResponse::User(user));
        }

        if bytes.starts_with(b"PASS ") {
            let pass: String = decode_latin1(&bytes[5..]).trim().to_string();
            return Ok(FtpResponse::Pass(pass));
        }

        if bytes.starts_with(b"SYST") {
            return Ok(FtpResponse::Syst);
        }

        if bytes.starts_with(b"NOOP") {
            return Ok(FtpResponse::Noop);
        }

        if bytes.starts_with(b"QUIT") {
            return Ok(FtpResponse::Quit);
        }

        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid response",
        ))
    }
}

#[inline]
fn decode_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}
