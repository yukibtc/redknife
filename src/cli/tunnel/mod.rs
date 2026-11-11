use std::collections::HashSet;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::str::FromStr;

use clap::Subcommand;

mod connect;
mod listen;

use crate::host_and_port::HostAndPort;
use crate::tunnel::TunnelTlsFingerprint;

const DEFAULT_SOCKS5: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1080));

#[derive(Debug, Clone)]
pub(super) struct Forward {
    bind: SocketAddr,
    target: HostAndPort,
}

impl FromStr for Forward {
    type Err = io::Error;

    fn from_str(forward: &str) -> Result<Self, Self::Err> {
        let split_at: Vec<usize> = match forward.find(']') {
            // Bracketed IPv6 bind: candidate splits only after it.
            Some(end) => forward[end..]
                .match_indices(':')
                .map(|(index, _)| end + index)
                .collect(),
            None => forward.match_indices(':').map(|(index, _)| index).collect(),
        };

        for index in split_at {
            let (Some(bind), Some(target)) = (forward.get(..index), forward.get(index + 1..))
            else {
                continue;
            };

            if let (Ok(bind), Ok(target)) = (bind.parse(), target.parse()) {
                return Ok(Self { bind, target });
            }
        }

        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid forward specification, expected <bind>:<target>: {forward}"),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ReversePort {
    Single(u16),
    Range { start: u16, end: u16 },
}

impl FromStr for ReversePort {
    type Err = io::Error;

    fn from_str(spec: &str) -> Result<Self, Self::Err> {
        let parse = |value: &str| {
            value.parse::<u16>().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "invalid reverse port specification, expected <port> or <start>-<end>: {spec}"
                    ),
                )
            })
        };

        match spec.split_once('-') {
            // Inclusive range, e.g. "8000-8010"
            Some((start, end)) => {
                let (start, end): (u16, u16) = (parse(start)?, parse(end)?);

                if start > end {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("inverted reverse port range: {spec}"),
                    ));
                }

                if start == end {
                    return Ok(Self::Single(start));
                }

                Ok(Self::Range { start, end })
            }
            // Single port
            None => Ok(Self::Single(parse(spec)?)),
        }
    }
}

impl IntoIterator for ReversePort {
    type Item = u16;
    type IntoIter = Box<dyn Iterator<Item = u16>>;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            Self::Single(port) => Box::new(std::iter::once(port)),
            Self::Range { start, end } => Box::new(start..=end),
        }
    }
}

#[derive(Debug, Subcommand)]
pub(super) enum TunnelCommand {
    /// Listen for tunnel connections and expose the target network
    #[command(arg_required_else_help = true)]
    Listen {
        /// Address to listen on for tunnel connections (<ip>:<port>)
        addr: SocketAddr,
        /// Auth token (auto-generated and printed if omitted)
        #[arg(long)]
        auth: Option<String>,
        /// Address of the SOCKS5 proxy to expose (<ip>:<port>)
        #[arg(long, default_value_t = DEFAULT_SOCKS5)]
        socks: SocketAddr,
        /// Disable the SOCKS5 proxy
        #[arg(long)]
        no_socks: bool,
        /// Expose a target service on a local operator port (repeatable)
        ///
        /// Connections accepted on <bind> are tunneled to <target>, dialed by
        /// the target side.
        ///
        /// Example: `--forward 127.0.0.1:3306:db.internal:3306` makes the target's
        /// MySQL reachable on the operator's 127.0.0.1:3306.
        #[arg(long = "forward", value_name = "BIND:TARGET")]
        forwards: Vec<Forward>,
        /// Allow reverse streams for <port> or <start>-<end> ranges, dialed on
        /// this machine's loopback (repeatable)
        ///
        /// The target side exposes the door with `--local-listen` on the same
        /// port. Undeclared ports are refused.
        #[arg(long = "reverse", value_name = "PORT")]
        reverse_ports_whitelist: Vec<ReversePort>,
        /// Custom directory where to store the tunnel certificate
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Connect a target network back to the operator
    #[command(arg_required_else_help = true, visible_alias = "conn")]
    Connect {
        /// Operator address to connect to (<host>:<port>)
        peer: HostAndPort,
        /// Auth token (must match the operator's)
        #[arg(long)]
        auth: String,
        /// Expected TLS certificate fingerprint (SHA256, hex)
        #[arg(long, required_unless_present = "tls_skip_verify")]
        fingerprint: Option<TunnelTlsFingerprint>,
        /// Skip TLS certificate verification (insecure)
        #[arg(long, conflicts_with = "fingerprint")]
        tls_skip_verify: bool,
        /// Number of dial workers
        #[arg(long, default_value_t = 50)]
        dial_workers: usize,
        /// Expose a local listener whose connections are delivered to the
        /// operator's loopback, on the same port (repeatable)
        ///
        /// The port must be declared with `--reverse` on the operator, or the
        /// streams are refused.
        ///
        /// Example: reverse shell callbacks — run the handler (i.e. `nc -lvnp 4444`)
        /// on the operator and point the payload at this address.
        #[arg(long = "local-listen", value_name = "ADDR")]
        local_listen: Vec<SocketAddr>,
    },
}

impl TunnelCommand {
    pub(super) async fn run(self) -> io::Result<()> {
        match self {
            Self::Listen {
                addr,
                auth,
                socks,
                no_socks,
                forwards,
                reverse_ports_whitelist,
                data_dir,
            } => {
                let mut ports: HashSet<u16> = HashSet::new();

                for port in reverse_ports_whitelist {
                    ports.extend(port);
                }

                listen::run(addr, auth, socks, no_socks, forwards, ports, data_dir).await
            }
            Self::Connect {
                peer,
                auth,
                fingerprint,
                tls_skip_verify: _,
                dial_workers,
                local_listen,
            } => connect::run(peer, auth, fingerprint, dial_workers, local_listen).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_forward_specifications() {
        let Forward { bind, target } =
            Forward::from_str("127.0.0.1:3306:10.200.180.151:3306").unwrap();
        assert_eq!(bind.to_string(), "127.0.0.1:3306");
        assert_eq!(target.to_string(), "10.200.180.151:3306");

        let Forward { bind, target } = Forward::from_str("0.0.0.0:8443:db.internal:443").unwrap();
        assert_eq!(bind.to_string(), "0.0.0.0:8443");
        assert_eq!(target.to_string(), "db.internal:443");

        let Forward { bind, target } = Forward::from_str("[::1]:3306:10.0.0.5:3306").unwrap();
        assert_eq!(bind.to_string(), "[::1]:3306");
        assert_eq!(target.to_string(), "10.0.0.5:3306");
    }

    #[test]
    fn rejects_forward_specifications() {
        assert!(Forward::from_str("127.0.0.1:3306").is_err());
        assert!(Forward::from_str("3306:10.200.180.151:3306").is_err());
        assert!(Forward::from_str("db.internal:3306").is_err());
    }

    #[test]
    fn parses_reverse_port_specifications() {
        assert_eq!(
            ReversePort::from_str("4444").unwrap(),
            ReversePort::Single(4444)
        );
        assert_eq!(
            ReversePort::from_str("8000-8010").unwrap(),
            ReversePort::Range {
                start: 8000,
                end: 8010
            }
        );

        // A single-port range is normalized to a single port.
        assert_eq!(
            ReversePort::from_str("4444-4444").unwrap(),
            ReversePort::Single(4444)
        );
    }

    #[test]
    fn rejects_reverse_port_specifications() {
        assert!(ReversePort::from_str("").is_err());
        assert!(ReversePort::from_str("http").is_err());
        assert!(ReversePort::from_str("4444-").is_err());
        assert!(ReversePort::from_str("-4444").is_err());
        assert!(ReversePort::from_str("8010-8000").is_err());
    }

    #[test]
    fn expands_reverse_port_specifications() {
        let mut ports: HashSet<u16> = HashSet::new();

        ports.extend(ReversePort::from_str("4444").unwrap());
        ports.extend(ReversePort::from_str("8000-8003").unwrap());

        assert_eq!(ports, HashSet::from([4444, 8000, 8001, 8002, 8003]));
    }
}
