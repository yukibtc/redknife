use clap::{Parser, Subcommand};

mod http;
mod ldap;

use self::http::HttpCommand;
use self::ldap::LdapCommand;

#[derive(Debug, Subcommand)]
enum Command {
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
            Command::Http { command } => command.run().await?,
            Command::Ldap { command } => command.run().await?,
        }

        Ok(())
    }
}
