use clap::{Parser, Subcommand};

mod command;

use self::command::http::HttpCommand;
use self::command::ldap::LdapCommand;

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
pub(super) struct Cli {
    #[command(subcommand)]
    command: Command,
}

impl Cli {
    pub(super) async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        match self.command {
            Command::Http { command } => match command {
                HttpCommand::Callback { ip, port } => {
                    command::http::callback::run(ip, port).await?
                }
                HttpCommand::Proxy {
                    ip,
                    port,
                    rules,
                    dump,
                    dump_format,
                } => command::http::proxy::run(ip, port, rules, dump, dump_format).await?,
                HttpCommand::Send {
                    target,
                    file,
                    output,
                } => command::http::send::run(target, file, output).await?,
            },
            Command::Ldap { command } => match command {
                LdapCommand::Serve { ip, port, raw } => {
                    command::ldap::serve::run(ip, port, raw).await?
                }
            },
        }

        Ok(())
    }
}

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
