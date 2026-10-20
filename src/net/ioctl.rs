use std::ffi::{CStr, CString, c_char, c_int, c_ushort};
use std::io::{self, ErrorKind};
use std::mem;
use std::net::Ipv4Addr;

use indexmap::IndexMap;
use indexmap::map::Entry;
use ipnet::Ipv4Net;
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

fn get_fd() -> io::Result<c_int> {
    let fd: c_int = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };

    if fd < 0 {
        let err: io::Error = io::Error::last_os_error();
        return Err(io::Error::other(format!("socket(AF_INET) failed: {err}")));
    }

    Ok(fd)
}

/// List the interfaces with an IPv4 address configured, via ioctl(SIOCGIFCONF).
/// Returns the interface names with their associated IPv4 address.
pub(super) fn list_interfaces_ioctl() -> io::Result<IndexMap<String, Ipv4Addr>> {
    let fd: c_int = get_fd()?;

    // The SIOCGIFCONF ioctl does not report how much space it needs:
    // start with a small buffer and double it while it comes back full.
    let mut buf_len: usize = 8 * mem::size_of::<ifreq>();
    let mut buf: Vec<u8>;

    let ifc_len: usize = loop {
        buf = vec![0; buf_len];

        let mut ifc: libc::ifconf = libc::ifconf {
            ifc_len: buf_len as c_int,
            ifc_ifcu: libc::__c_anonymous_ifc_ifcu {
                ifcu_buf: buf.as_mut_ptr() as *mut c_char,
            },
        };

        if unsafe { libc::ioctl(fd, libc::SIOCGIFCONF, &mut ifc) } < 0 {
            let err: io::Error = io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(io::Error::other(format!("SIOCGIFCONF failed: {err}")));
        }

        let len: usize = ifc.ifc_len as usize;

        if len < buf_len {
            break len;
        }

        buf_len *= 2;
    };

    unsafe { libc::close(fd) };

    let reqs: *const ifreq = buf.as_ptr() as *const ifreq;
    let count: usize = ifc_len / mem::size_of::<ifreq>();

    let mut ifaces: IndexMap<String, Ipv4Addr> = IndexMap::with_capacity(count);

    for i in 0..count {
        let req: &ifreq = unsafe { &*reqs.add(i) };

        let name: String = unsafe { CStr::from_ptr(req.ifr_name.as_ptr()) }
            .to_string_lossy()
            .into_owned();

        // Skip entries without an IPv4 address (e.g. IPv6-only aliases)
        if unsafe { req.ifr_ifru.ifru_addr.sa_family } != libc::AF_INET as c_ushort {
            continue;
        }

        // ifr_addr is a sockaddr_in: sa_data is port (2 bytes, network order)
        // followed by the IPv4 address (4 bytes, network order)
        let sa_data: [u8; 14] = unsafe { req.ifr_ifru.ifru_addr.sa_data }.map(|b| b as u8);

        let ip: Ipv4Addr = Ipv4Addr::new(sa_data[2], sa_data[3], sa_data[4], sa_data[5]);

        // The same interface may appear multiple times (one entry per address)
        if let Entry::Vacant(e) = ifaces.entry(name) {
            e.insert(ip);
        }
    }

    Ok(ifaces)
}

pub(super) fn read_mac_addr_ioctl(iface: &str) -> io::Result<MacAddr> {
    let mut ifr: ifreq = get_ifr(iface)?;

    let fd: c_int = get_fd()?;

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

    let fd: c_int = get_fd()?;

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

/// Read the IPv4 network for a given interface via ioctl(SIOCGIFADDR)
/// and ioctl(SIOCGIFNETMASK)
pub fn read_ipv4_netmask(iface: &str) -> io::Result<Ipv4Net> {
    let mut ifr: ifreq = get_ifr(iface)?;

    let fd: c_int = get_fd()?;

    if unsafe { libc::ioctl(fd, libc::SIOCGIFNETMASK, &mut ifr as *mut ifreq) } < 0 {
        let err: io::Error = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(io::Error::other(format!(
            "SIOCGIFNETMASK failed for '{iface}': {err}"
        )));
    }

    unsafe { libc::close(fd) };

    let sa_family: c_ushort = unsafe { ifr.ifr_ifru.ifru_netmask.sa_family };

    // Reject interfaces without an IPv4 netmask (e.g. unconfigured ones)
    if sa_family != libc::AF_INET as c_ushort {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("interface '{iface}' has no IPv4 netmask (sa_family {sa_family})"),
        ));
    }

    // ifr_netmask is a sockaddr_in: sa_data is port (2 bytes, network order)
    // followed by the IPv4 netmask (4 bytes, network order)
    let sa_data: [u8; 14] = unsafe { ifr.ifr_ifru.ifru_netmask.sa_data }.map(|b| b as u8);

    let mask: Ipv4Addr = Ipv4Addr::new(sa_data[2], sa_data[3], sa_data[4], sa_data[5]);

    let prefix: u8 = mask
        .octets()
        .into_iter()
        .map(|b| b.count_ones() as u8)
        .sum();

    let ip: Ipv4Addr = read_ipv4(iface)?;

    Ipv4Net::new(ip, prefix).map_err(io::Error::other)
}
