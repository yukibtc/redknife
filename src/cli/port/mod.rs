use std::net::SocketAddr;

use clap::Subcommand;

mod forward;

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum PortCommand {
    /// Forward TCP connections from a listener to target
    #[command(arg_required_else_help = true)]
    Forward {
        /// Address to listen on (<ip>:<port>, e.g. 0.0.0.0:33060)
        listen: SocketAddr,
        /// Target to forward connections to (<host>:<port>, e.g. 172.16.0.10:3306)
        target: String,
    },
}

impl PortCommand {
    pub(in crate::cli) async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Forward { listen, target } => forward::run(listen, target).await?,
        }

        Ok(())
    }
}
