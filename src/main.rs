use std::process::ExitCode;

use clap::Parser;

#[cfg(target_os = "linux")]
mod arp;
mod cli;
mod ftp;
mod hexdump;
mod http;
mod ldap;
#[cfg(target_os = "linux")]
mod net;

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
