use std::process::ExitCode;

use clap::Parser;

mod cli;
mod command;
mod http;
mod ldap;
mod tui;

use self::cli::{Cli, Command, HttpCommand, LdapCommand};

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Cli = Cli::parse();

    match args.command {
        Command::Http { command } => match command {
            HttpCommand::Callback { ip, port } => command::http::callback::run(ip, port).await?,
            HttpCommand::Proxy { ip, port, out_dir } => {
                command::http::proxy::run(ip, port, out_dir).await?
            }
            HttpCommand::Send {
                target,
                file,
                interactive,
                output,
            } => command::http::send::run(target, file, interactive, output).await?,
        },
        Command::Ldap { command } => match command {
            LdapCommand::Serve { ip, port, raw } => {
                command::ldap::serve::run(ip, port, raw).await?
            }
        },
    }

    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("Error: {err}");
            ExitCode::FAILURE
        }
    }
}
