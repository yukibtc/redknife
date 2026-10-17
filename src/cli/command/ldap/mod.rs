use std::net::{IpAddr, Ipv4Addr};

use clap::Subcommand;

pub mod serve;

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum LdapCommand {
    /// Starts a minimal LDAP server.
    Serve {
        /// Local IP address to listen on
        #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)))]
        ip: IpAddr,
        /// Local TCP port to listen on
        #[arg(long, default_value_t = 389)]
        port: u16,
        /// Show complete incoming BER messages in addition to decoded fields
        #[arg(long)]
        raw: bool,
    },
}
