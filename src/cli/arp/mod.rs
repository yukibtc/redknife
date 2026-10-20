use std::io;
use std::net::Ipv4Addr;
use std::path::PathBuf;

use clap::Subcommand;

use crate::net::{MacAddr, list_interfaces};

mod resolve;
mod spoof;

#[derive(Debug, Subcommand)]
pub(super) enum ArpCommand {
    /// Resolve an IPv4
    #[command(arg_required_else_help = true)]
    Resolve {
        /// IP address to resolve
        ip: Ipv4Addr,
        /// Network interface to use
        #[arg(long)]
        iface: Option<String>,
    },
    /// Starts an ARP spoofer
    #[command(arg_required_else_help = true)]
    Spoof {
        /// IP address of the impersonated gateway
        gateway_ip: Ipv4Addr,
        /// MAC address of the impersonated gateway
        #[arg(long)]
        gateway_mac: Option<MacAddr>,
        /// IP address of the target. If not specified, the spoof will be in broadcast mode.
        #[arg(long)]
        target_ip: Option<Ipv4Addr>,
        /// MAC address of the target. If not specified, the spoofer will attempt to resolve it automatically.
        #[arg(long, requires = "target_ip")]
        target_mac: Option<MacAddr>,
        /// Network interface to use
        #[arg(long)]
        iface: Option<String>,
        /// Rate at which ARP packets are sent in milliseconds
        #[arg(long, default_value_t = 500)]
        rate: u64,
        /// Number of packets to send initially
        #[arg(long, default_value_t = 5)]
        burst: usize,
        /// Path to a pcap file where the spoofed traffic (ARP and forwarded IPv4 packets) will be dumped
        #[arg(long)]
        dump: Option<PathBuf>,
    },
}

impl ArpCommand {
    pub(super) async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Resolve { iface, ip } => {
                let iface: String = detect_iface(iface)?;
                resolve::run(ip, iface).await?;
            }
            Self::Spoof {
                iface,
                gateway_ip,
                gateway_mac,
                target_ip,
                target_mac,
                rate,
                burst,
                dump,
            } => {
                let iface: String = detect_iface(iface)?;
                spoof::run(
                    gateway_ip,
                    gateway_mac,
                    target_ip,
                    target_mac,
                    iface,
                    rate,
                    burst,
                    dump,
                )
                .await?;
            }
        }

        Ok(())
    }
}

fn detect_iface(iface: Option<String>) -> io::Result<String> {
    match iface {
        Some(iface) => Ok(iface),
        None => list_interfaces()?
            .into_iter()
            .next()
            .ok_or_else(|| io::Error::other("No active network interface found")),
    }
}
