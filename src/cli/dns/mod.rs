use std::net::IpAddr;

use clap::Subcommand;

mod query;

use crate::dns::{DnsType, DomainName};

#[derive(Debug, Subcommand)]
pub(super) enum DnsCommand {
    /// DNS query
    Query {
        /// Domain name to query
        domain: DomainName,
        /// Record type
        r#type: Option<DnsType>,
        /// Server IP to use for the query
        #[arg(long)]
        ip: Option<IpAddr>,
        /// Server port to use for the query
        #[arg(long, default_value_t = 53)]
        port: u16,
        /// Timeout in seconds
        #[arg(long, default_value_t = 60)]
        timeout: u64,
    },
}

impl DnsCommand {
    pub(super) async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Query {
                domain,
                r#type,
                ip,
                port,
                timeout,
            } => query::run(domain, r#type, ip, port, timeout).await?,
        }

        Ok(())
    }
}
