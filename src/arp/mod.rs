mod packet;
mod resolve;

pub use self::packet::{ArpOpCode, ArpPacket};
pub use self::resolve::{collect_resolved_macs, probe_resolve_mac, resolve_single_mac};
use crate::net::DataLinkFrame;

pub type ArpFrame = DataLinkFrame<ArpPacket>;
