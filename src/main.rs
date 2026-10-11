use std::process::ExitCode;

use clap::Parser;

mod cli;
mod command;

use self::cli::{Cli, Command, HttpCommand};

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Cli = Cli::parse();

    match args.command {
        Command::Http { command } => match command {
            HttpCommand::Callback { ip, port } => command::http::callback::run(ip, port).await?,
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
