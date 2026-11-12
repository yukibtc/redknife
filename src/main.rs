use std::process::ExitCode;

use clap::Parser;

#[cfg(target_os = "linux")]
mod arp;
mod cli;
mod config_loader;
mod dir;
mod dns;
mod ftp;
mod hexdump;
mod host_and_port;
mod http;
mod ldap;
mod logger;
#[cfg(target_os = "linux")]
mod net;
#[cfg(target_os = "linux")]
mod proc;
mod socks;
mod tunnel;

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
