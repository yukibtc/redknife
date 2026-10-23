use std::net::Ipv4Addr;

use bytes::Bytes;
use chrono::{DateTime, Local};

use crate::net::MacAddr;

pub(super) type PoisonKey = (Ipv4Addr, Ipv4Addr);

#[derive(Clone, Copy)]
pub(super) struct ArpInfo {
    pub(super) sender_mac: MacAddr,
    pub(super) sender_ip: Ipv4Addr,
    pub(super) target_mac: MacAddr,
    pub(super) target_ip: Ipv4Addr,
}

impl ArpInfo {
    /// Identifies a poison announcement: the victim told and the impersonated IP.
    #[inline]
    pub(super) fn poison_key(&self) -> PoisonKey {
        (self.target_ip, self.sender_ip)
    }
}

#[derive(Clone, Copy)]
pub(super) enum Transport {
    Tcp { src_port: u16, dst_port: u16 },
    Udp { src_port: u16, dst_port: u16 },
    Icmp,
    Other(u8),
}

#[derive(Clone)]
pub(super) enum SpoofEventKind {
    /// Poison frame sent: we announced that `sender_ip` is at `sender_mac`
    Poison(ArpInfo),
    /// ARP request sniffed from the network
    ArpRequest(ArpInfo),
    /// ARP reply sniffed from the network
    ArpReply(ArpInfo),
    /// ARP cache restoration frame sent
    Restore(ArpInfo),
    /// IPv4 packet forwarded between the poisoned hosts
    Forwarded {
        src_ip: Ipv4Addr,
        dst_ip: Ipv4Addr,
        transport: Transport,
    },
}

#[derive(Clone)]
pub(super) struct SpoofEvent {
    pub(super) timestamp: DateTime<Local>,
    pub(super) kind: SpoofEventKind,
    pub(super) frame: Bytes,
}

impl SpoofEvent {
    pub(super) fn new(kind: SpoofEventKind, frame: Bytes) -> Self {
        Self {
            timestamp: Local::now(),
            kind,
            frame,
        }
    }

    #[inline]
    pub(super) fn frame(&self) -> &[u8] {
        &self.frame
    }
}
