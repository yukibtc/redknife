use std::net::IpAddr;
use std::{fs, io};

const RESOLV_CONF: &str = "/etc/resolv.conf";

pub(super) fn nameserver() -> io::Result<IpAddr> {
    let content: String = fs::read_to_string(RESOLV_CONF)?;

    parse(&content).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("no nameserver found in {RESOLV_CONF}"),
        )
    })
}

fn parse(content: &str) -> Option<IpAddr> {
    for line in content.lines() {
        let Some((_, nameserver)) = line.split_once("nameserver ") else {
            continue;
        };

        for token in nameserver.split_whitespace() {
            let token: &str = token.trim();

            if token.is_empty() {
                continue;
            }

            if let Ok(ip) = token.parse() {
                return Some(ip);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse() {
        let content = "# comment\nnameserver 10.0.0.1\nsearch example.com\n";
        assert_eq!(parse(content), Some(IpAddr::from([10, 0, 0, 1])));

        let content = "; semicolon comment\nnameserver 1.2.3.4 ; inline\n";
        assert_eq!(parse(content), Some(IpAddr::from([1, 2, 3, 4])));

        let content = "; semicolon comment\nnameserver 1.2.3.4 # inline\n";
        assert_eq!(parse(content), Some(IpAddr::from([1, 2, 3, 4])));

        let content = "nameserver  1.2.3.4   # inline\n";
        assert_eq!(parse(content), Some(IpAddr::from([1, 2, 3, 4])));

        let content = "nameserver 2001:db8::1\n";
        assert_eq!(
            parse(content),
            Some(IpAddr::from([0x2001, 0x0db8, 0, 0, 0, 0, 0, 1]))
        );

        let content = "search example.com\n";
        assert_eq!(parse(content), None);

        let content = "";
        assert_eq!(parse(content), None);

        let content = "nameserver not-an-ip\n";
        assert_eq!(parse(content), None);
    }
}
