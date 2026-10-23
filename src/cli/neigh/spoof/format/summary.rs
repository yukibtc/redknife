use std::{fmt, io};

use tokio::io::{AsyncWrite, AsyncWriteExt};

use super::super::event::{SpoofEvent, SpoofEventKind, Transport};

pub(super) struct SummaryOutput<'a> {
    event: &'a SpoofEvent,
}

impl SummaryOutput<'_> {
    pub(super) async fn write_and_flush<W>(&self, out: &mut W) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let output: String = format!("{self}\n");
        out.write_all(output.as_bytes()).await?;
        out.flush().await
    }
}

impl<'a> From<&'a SpoofEvent> for SummaryOutput<'a> {
    fn from(event: &'a SpoofEvent) -> Self {
        Self { event }
    }
}

impl fmt::Display for SummaryOutput<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let event: &SpoofEvent = self.event;

        write!(f, "[{}] ", event.timestamp.format("%Y-%m-%d %H:%M:%S"))?;

        match &event.kind {
            SpoofEventKind::Poison(info) => write!(
                f,
                "Poison: {} is-at {} (told {} [{}])",
                info.sender_ip, info.sender_mac, info.target_ip, info.target_mac,
            ),
            SpoofEventKind::ArpRequest(info) => write!(
                f,
                "ARP request: who-has {}? tell {} [{}]",
                info.target_ip, info.sender_ip, info.sender_mac,
            ),
            SpoofEventKind::ArpReply(info) => write!(
                f,
                "ARP reply: {} is-at {} (to {} [{}])",
                info.sender_ip, info.sender_mac, info.target_ip, info.target_mac,
            ),
            SpoofEventKind::Restore(info) => write!(
                f,
                "Restore: {} is-at {} (told {} [{}])",
                info.sender_ip, info.sender_mac, info.target_ip, info.target_mac,
            ),
            SpoofEventKind::Forwarded {
                src_ip,
                dst_ip,
                transport,
            } => match transport {
                Transport::Tcp { src_port, dst_port } => write!(
                    f,
                    "Forwarded TCP {src_ip}:{src_port} -> {dst_ip}:{dst_port}"
                ),
                Transport::Udp { src_port, dst_port } => write!(
                    f,
                    "Forwarded UDP {src_ip}:{src_port} -> {dst_ip}:{dst_port}"
                ),
                Transport::Icmp => write!(f, "Forwarded ICMP {src_ip} -> {dst_ip}"),
                Transport::Other(proto) => {
                    write!(f, "Forwarded IPv4/{proto} {src_ip} -> {dst_ip}")
                }
            },
        }
    }
}
