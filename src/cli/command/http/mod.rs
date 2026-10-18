use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use clap::Subcommand;

pub mod callback;
pub mod proxy;
pub mod send;

use self::proxy::dump::HttpProxyDumpFormat;
use crate::http::target::HttpTarget;

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum HttpCommand {
    /// Start an HTTP listener that logs incoming requests
    Callback {
        /// Local IP address to listen on
        #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)))]
        ip: IpAddr,
        /// Local TCP port to listen on
        #[arg(long, default_value_t = 8000)]
        port: u16,
    },
    /// Inspect, intercept, modify, and forward/drop HTTP traffic.
    ///
    /// Starts a local HTTP proxy that allows requests
    /// to be inspected, modified, forwarded, or dropped.
    ///
    /// Configure your browser or HTTP client to use the proxy.
    ///
    /// HTTPS is not supported yet.
    Proxy {
        /// Local IP address to bind the proxy to
        #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)))]
        ip: IpAddr,
        /// Local port to listen on
        #[arg(long, default_value_t = 8001)]
        port: u16,
        /// Select rules to filter and modify HTTP traffic
        #[arg(long)]
        rules: Option<PathBuf>,
        /// Dump requests to file
        #[arg(long)]
        dump: Option<PathBuf>,
        /// The format to use when dumping requests
        #[arg(long, default_value_t = HttpProxyDumpFormat::Text)]
        dump_format: HttpProxyDumpFormat,
    },
    /// Send a request
    #[command(arg_required_else_help = true)]
    Send {
        /// Target URL
        target: HttpTarget,
        /// Path to a saved HTTP request to repeat
        file: PathBuf,
        // /// Open the request file in the interactive HTTP editor
        // #[arg(short, long)]
        // interactive: bool,
        /// Save normalized response headers and unpacked body to this path instead of stdout
        //#[arg(short, long, requires = "file", conflicts_with = "interactive")]
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}
