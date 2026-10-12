use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::http::target::HttpTarget;

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
    /// Intercept, inspect, and modify HTTP traffic.
    ///
    /// Starts a local HTTP interception proxy that allows requests
    /// to be inspected, modified, forwarded, or dropped.
    ///
    /// Configure your browser or HTTP client to use the proxy.
    /// HTTPS is not supported.
    Intercept {
        /// Local IP address to bind the proxy to
        #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)))]
        ip: IpAddr,
        /// Local port to listen on
        #[arg(long, default_value_t = 8001)]
        port: u16,
        /// Directory for manually saved HTTP requests (defaults to the current directory).
        #[arg(long)]
        out_dir: Option<PathBuf>,
    },
    /// Repeat a saved HTTP request
    #[command(arg_required_else_help = true)]
    Repeat {
        /// Target URL
        target: HttpTarget,
        // TODO: make this optional when we'll have a TUI for the repeater; None will mean open the TUI editor
        /// Path to a saved HTTP request to repeat
        file: PathBuf,
        /// Save response headers and decoded body to this path instead of stdout
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}
