use std::ffi::{CString, c_int, c_ushort};
use std::io::{self, ErrorKind};
use std::mem;
use std::net::Ipv4Addr;

use libc::{IF_NAMESIZE, ifreq};

use super::mac::MacAddr;
fn get_ifr(iface: &str) -> io::Result<ifreq> {
    let mut ifr: ifreq = unsafe { mem::zeroed() };

    let name: CString = CString::new(iface)?;
    if name.as_bytes_with_nul().len() > IF_NAMESIZE {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "interface name too long",
        ));
    }

    unsafe {
        libc::strncpy(ifr.ifr_name.as_mut_ptr(), name.as_ptr(), IF_NAMESIZE - 1);
    }

    Ok(ifr)
}

pub(super) fn read_mac_addr_ioctl(iface: &str) -> io::Result<MacAddr> {
    let mut ifr: ifreq = get_ifr(iface)?;

    let fd: c_int = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };

    if fd < 0 {
        let err: io::Error = io::Error::last_os_error();
        return Err(io::Error::other(format!("socket(AF_INET) failed: {err}")));
    }

    if unsafe { libc::ioctl(fd, libc::SIOCGIFHWADDR, &mut ifr as *mut ifreq) } < 0 {
        let err: io::Error = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(io::Error::other(format!(
            "SIOCGIFHWADDR failed for '{iface}': {err}"
        )));
    }

    unsafe { libc::close(fd) };

    // ifr_hwaddr is a sockaddr, the first 6 bytes of sa_data are the MAC
    let sa_data: [u8; 14] = unsafe { ifr.ifr_ifru.ifru_hwaddr.sa_data }.map(|b| b as u8);
    let sa_family: c_ushort = unsafe { ifr.ifr_ifru.ifru_hwaddr.sa_family };

    // Reject interfaces without an Ethernet MAC address (e.g. loopback),
    // otherwise we would silently return an all-zero MAC
    if sa_family != libc::ARPHRD_ETHER as c_ushort {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("interface '{iface}' has no MAC address (sa_family {sa_family})"),
        ));
    }

    Ok(MacAddr::new([
        sa_data[0], sa_data[1], sa_data[2], sa_data[3], sa_data[4], sa_data[5],
    ]))
}

/// Read the IPv4 address for a given interface via ioctl(SIOCGIFADDR)
pub(super) fn read_ipv4(iface: &str) -> io::Result<Ipv4Addr> {
    let mut ifr: ifreq = get_ifr(iface)?;

    let fd: c_int = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };

    if fd < 0 {
        let err: io::Error = io::Error::last_os_error();
        return Err(io::Error::other(format!("socket(AF_INET) failed: {err}")));
    }

    if unsafe { libc::ioctl(fd, libc::SIOCGIFADDR, &mut ifr as *mut ifreq) } < 0 {
        let err: io::Error = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(io::Error::other(format!(
            "SIOCGIFADDR failed for '{iface}': {err}"
        )));
    }

    unsafe { libc::close(fd) };

    let sa_family: c_ushort = unsafe { ifr.ifr_ifru.ifru_addr.sa_family };

    // Reject interfaces without an IPv4 address (e.g. unconfigured ones),
    // otherwise we would silently return 0.0.0.0
    if sa_family != libc::AF_INET as c_ushort {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("interface '{iface}' has no IPv4 address (sa_family {sa_family})"),
        ));
    }

    // ifr_addr is a sockaddr_in: sa_data is port (2 bytes, network order)
    // followed by the IPv4 address (4 bytes, network order)
    let sa_data: [u8; 14] = unsafe { ifr.ifr_ifru.ifru_addr.sa_data }.map(|b| b as u8);

    Ok(Ipv4Addr::new(
        sa_data[2], sa_data[3], sa_data[4], sa_data[5],
    ))
}
