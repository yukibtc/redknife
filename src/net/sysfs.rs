use std::io::ErrorKind;
use std::net::Ipv4Addr;
use std::{fs, io};

use crate::net::MacAddr;

const PROC_NET_ROUTE: &str = "/proc/net/route";

pub(super) fn read_mac_addr_sysfs(iface: &str) -> io::Result<MacAddr> {
    let path: String = format!("/sys/class/net/{iface}/address");

    let content: String = fs::read_to_string(&path)
        .map_err(|e| io::Error::new(e.kind(), format!("cannot read '{path}': {e}")))?;

    let mac: MacAddr = content.trim().parse().map_err(|e| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("invalid MAC address in '{path}': {e}"),
        )
    })?;

    Ok(mac)
}

/// Read the default IPv4 gateway for a given interface from `/proc/net/route`.
pub fn read_ipv4_gateway_sysfs(iface: &str) -> io::Result<Ipv4Addr> {
    let content: String = fs::read_to_string(PROC_NET_ROUTE)?;

    for line in content.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();

        if fields.len() < 3 {
            continue;
        }

        if fields[0] != iface || fields[1] != "00000000" || fields[2] == "00000000" {
            continue;
        }

        let gateway: u32 = u32::from_str_radix(fields[2], 16)
            .map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?;

        // Converts an integer from big endian to the target's endianness.
        // On big endian this is a no-op. On little endian the bytes are swapped.
        let ip: u32 = u32::from_be(gateway);

        return Ok(Ipv4Addr::from(ip));
    }

    Err(io::Error::new(
        ErrorKind::NotFound,
        format!("no IPv4 default gateway for interface '{iface}'"),
    ))
}
