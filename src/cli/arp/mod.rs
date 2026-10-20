use std::net::Ipv4Addr;
use std::path::PathBuf;

use clap::Subcommand;

use crate::net::MacAddr;

mod spoof;

#[derive(Debug, Subcommand)]
pub(super) enum ArpCommand {
    /// Starts an ARP spoofer
    #[command(arg_required_else_help = true)]
    Spoof {
        /// Network interface to use
        iface: String,
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
                spoof::run(
                    iface,
                    gateway_ip,
                    gateway_mac,
                    target_ip,
                    target_mac,
                    rate,
                    burst,
                    dump,
                )
                .await?
            }
        }

        Ok(())
    }
}
