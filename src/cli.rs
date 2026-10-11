use std::net::{IpAddr, Ipv4Addr};

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// HTTP utilities for receiving and inspecting requests
    #[command(arg_required_else_help = true)]
    Http {
        #[command(subcommand)]
        command: HttpCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum HttpCommand {
    /// Start an HTTP listener that logs incoming requests
    Callback {
        /// Local IP address to listen on
        #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)))]
        ip: IpAddr,
        /// Local TCP port to listen on
        #[arg(long, default_value_t = 8000)]
        port: u16,
    },
}
