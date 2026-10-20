use clap::{Parser, Subcommand};

#[cfg(target_os = "linux")]
mod arp;
mod ftp;
mod http;
mod ldap;

#[cfg(target_os = "linux")]
use self::arp::ArpCommand;
use self::ftp::FtpCommand;
use self::http::HttpCommand;
use self::ldap::LdapCommand;

#[derive(Debug, Subcommand)]
enum Command {
    /// ARP utilities
    #[cfg(target_os = "linux")]
    #[command(arg_required_else_help = true)]
    Arp {
        #[command(subcommand)]
        command: ArpCommand,
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
}

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
pub(super) struct Cli {
    #[command(subcommand)]
    command: Command,
}

impl Cli {
    pub(super) async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        match self.command {
            #[cfg(target_os = "linux")]
            Command::Arp { command } => command.run().await?,
            Command::Ftp { command } => command.run().await?,
            Command::Http { command } => command.run().await?,
            Command::Ldap { command } => command.run().await?,
        }

        Ok(())
    }
}
