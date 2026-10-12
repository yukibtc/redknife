use std::process::ExitCode;

use clap::Parser;

mod cli;
mod command;
mod http;
mod proxy;

use self::cli::{Cli, Command, HttpCommand};

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Cli = Cli::parse();

    match args.command {
        Command::Http { command } => match command {
            HttpCommand::Callback { ip, port } => command::http::callback::run(ip, port).await?,
            HttpCommand::Intercept { ip, port, out_dir } => {
                command::http::intercept::run(ip, port, out_dir).await?
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
            eprintln!("Error: {}", err);
            ExitCode::FAILURE
        }
    }
}
