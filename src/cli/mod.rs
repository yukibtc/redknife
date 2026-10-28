use clap::{Parser, Subcommand};

mod codec;
mod dns;
mod ftp;
mod http;
mod ldap;
#[cfg(target_os = "linux")]
mod neigh;

use self::codec::CodecCommand;
use self::dns::DnsCommand;
use self::ftp::FtpCommand;
use self::http::HttpCommand;
use self::ldap::LdapCommand;
#[cfg(target_os = "linux")]
use self::neigh::NeighCommand;

#[derive(Debug, Subcommand)]
enum Command {
    /// Codec & Crypto utilities
    #[command(arg_required_else_help = true)]
    Codec {
        #[command(subcommand)]
        command: CodecCommand,
    },
    /// DNS utilities
    #[command(arg_required_else_help = true)]
    Dns {
        #[command(subcommand)]
        command: DnsCommand,
    },
    /// FTP utilities
    #[command(arg_required_else_help = true)]
    Ftp {
        #[command(subcommand)]
        command: FtpCommand,
    },
    /// HTTP utilities
    #[command(arg_required_else_help = true)]
    Http {
        #[command(subcommand)]
        command: HttpCommand,
    },
    /// LDAP utilities
    #[command(arg_required_else_help = true)]
    Ldap {
        #[command(subcommand)]
        command: LdapCommand,
    },
    /// Neigh (ARP/NDP) utilities
    #[cfg(target_os = "linux")]
    #[command(arg_required_else_help = true)]
    Neigh {
        #[command(subcommand)]
        command: NeighCommand,
    },
}

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
pub(super) struct Cli {
    #[command(subcommand)]
    command: Command,
}

impl Cli {
    // TODO: create tokio runtime only for async commands
    pub(super) async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        match self.command {
            Command::Codec { command } => command.run()?,
            Command::Dns { command } => command.run().await?,
            Command::Ftp { command } => command.run().await?,
            Command::Http { command } => command.run().await?,
            Command::Ldap { command } => command.run().await?,
            #[cfg(target_os = "linux")]
            Command::Neigh { command } => command.run().await?,
        }

        Ok(())
    }
}
