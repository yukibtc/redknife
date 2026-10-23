use std::str::FromStr;
use std::{fmt, io};

use clap::ValueEnum;
use tokio::io::AsyncWrite;

use crate::net::PcapWriter;

mod summary;

use self::summary::SummaryOutput;
use super::event::SpoofEvent;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(in crate::cli) enum NeighSpoofFormat {
    Summary,
    Pcap,
}

impl fmt::Display for NeighSpoofFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Summary => write!(f, "summary"),
            Self::Pcap => write!(f, "pcap"),
        }
    }
}

impl FromStr for NeighSpoofFormat {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "summary" => Ok(Self::Summary),
            "pcap" => Ok(Self::Pcap),
            _ => Err("unsupported stdout format"),
        }
    }
}

pub(super) enum NeighSpoofWriter<T> {
    Summary(T),
    Pcap(PcapWriter<T>),
}

impl<T> NeighSpoofWriter<T>
where
    T: AsyncWrite + Unpin,
{
    pub(super) async fn write(&mut self, event: &SpoofEvent) -> io::Result<()> {
        match self {
            Self::Summary(out) => SummaryOutput::from(event).write_and_flush(out).await,
            Self::Pcap(out) => out.write_packet(event.frame()).await,
        }
    }
}
