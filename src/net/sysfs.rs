use std::io::ErrorKind;
use std::{fs, io};

use crate::net::MacAddr;

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
