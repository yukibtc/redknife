mod packet;
mod resolve;

pub use self::packet::{ArpOpCode, ArpPacket};
pub use self::resolve::resolve_mac;
use crate::net::DataLinkFrame;

pub type ArpFrame = DataLinkFrame<ArpPacket>;
