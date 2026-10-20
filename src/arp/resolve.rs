use std::net::Ipv4Addr;
use std::time::Duration;

use tokio::{io, time};

use super::ArpFrame;
use super::packet::{ArpOpCode, ArpPacket};
use crate::net::{DataLinkReceiver, DataLinkSender, Host, MacAddr};

const TIMEOUT: Duration = Duration::from_secs(3);

pub async fn resolve_mac(
    tx: &DataLinkSender,
    rx: &mut DataLinkReceiver,
    sender: Host,
    target_ip: Ipv4Addr,
) -> io::Result<Option<MacAddr>> {
    let req = ArpPacket {
        opcode: ArpOpCode::Request,
        sender_mac: sender.mac(),
        sender_ip: sender.ip(),
        target_mac: MacAddr::UNSPECIFIED,
        target_ip,
    };

    let frame: ArpFrame = ArpFrame::new(MacAddr::BROADCAST, sender.mac(), req);

    tx.send(&frame.encode()).await?;

    let deadline = time::sleep(TIMEOUT);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = &mut deadline => return Ok(None),
            frame = rx.next() => {
                let frame: Vec<u8> = frame?;

                let Ok(arp) = ArpFrame::decode(&frame) else {
                    continue;
                };

                // Discard our own frames
                if arp.eth_src == sender.mac() {
                    continue;
                }

                if is_valid_reply(&arp, sender.mac(), target_ip) {
                    return Ok(Some(arp.sender_mac));
                }
            }
        }
    }
}

/// Returns `true` if the packet is a valid ARP reply to our request.
fn is_valid_reply(arp: &ArpPacket, our_mac: MacAddr, target_ip: Ipv4Addr) -> bool {
    arp.opcode.is_reply()
        && arp.sender_ip == target_ip
        && arp.sender_mac != our_mac
        && !arp.sender_mac.is_broadcast()
        && !arp.sender_mac.is_unspecified()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUR_MAC: MacAddr = MacAddr::new([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
    const IP: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 1);

    fn packet(opcode: ArpOpCode, sender_mac: MacAddr, sender_ip: Ipv4Addr) -> ArpPacket {
        ArpPacket {
            opcode,
            sender_mac,
            sender_ip,
            target_mac: MacAddr::UNSPECIFIED,
            target_ip: Ipv4Addr::UNSPECIFIED,
        }
    }

    #[test]
    fn valid_reply() {
        let mac = MacAddr::new([0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
        assert!(is_valid_reply(
            &packet(ArpOpCode::Reply, mac, IP),
            OUR_MAC,
            IP
        ));
    }

    #[test]
    fn ignores_request() {
        let mac = MacAddr::new([0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
        assert!(!is_valid_reply(
            &packet(ArpOpCode::Request, mac, IP),
            OUR_MAC,
            IP
        ));
    }

    #[test]
    fn ignores_wrong_ip() {
        let mac = MacAddr::new([0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
        assert!(!is_valid_reply(
            &packet(ArpOpCode::Reply, mac, Ipv4Addr::new(10, 0, 0, 1)),
            OUR_MAC,
            IP
        ));
    }

    #[test]
    fn ignores_our_own_packets() {
        assert!(!is_valid_reply(
            &packet(ArpOpCode::Reply, OUR_MAC, IP),
            OUR_MAC,
            IP
        ));
    }

    #[test]
    fn ignores_broadcast_sender() {
        assert!(!is_valid_reply(
            &packet(ArpOpCode::Reply, MacAddr::BROADCAST, IP),
            OUR_MAC,
            IP
        ));
    }

    #[test]
    fn ignores_unspecified_sender() {
        assert!(!is_valid_reply(
            &packet(ArpOpCode::Reply, MacAddr::UNSPECIFIED, IP),
            OUR_MAC,
            IP
        ));
    }
}
