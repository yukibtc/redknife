use std::process::ExitCode;

use clap::Parser;

mod cli;
mod http;
mod ldap;

use self::cli::Cli;

#[inline]
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Cli = Cli::parse();
    args.run().await?;
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
