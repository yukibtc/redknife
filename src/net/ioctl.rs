//! Network interface info via the Linux `netdevice` ioctls.
//!
//! https://man7.org/linux/man-pages/man7/netdevice.7.html

use std::ffi::{CStr, c_char, c_int, c_ushort};
use std::io::{self, ErrorKind};
use std::mem;
use std::net::Ipv4Addr;
use std::os::fd::OwnedFd;

use indexmap::IndexMap;
use indexmap::map::Entry;
use ipnet::Ipv4Net;
use rustix::ioctl::{Opcode, Updater, ioctl};
use rustix::net::{self, AddressFamily, SocketType};

use super::mac::MacAddr;

const IF_NAMESIZE: usize = 16;

// The `sa_family` of `ifr_hwaddr` from `<linux/if_arp.h>`
const ARPHRD_ETHER: c_ushort = 1;

// The SIOC* ioctl request numbers, Linux-wide constants from `include/uapi/linux/sockios.h`
const SIOCGIFCONF: Opcode = 0x8912;
const SIOCGIFADDR: Opcode = 0x8915;
const SIOCGIFNETMASK: Opcode = 0x891b;
const SIOCGIFHWADDR: Opcode = 0x8927;

#[repr(C)]
#[derive(Clone, Copy)]
struct sockaddr {
    sa_family: c_ushort,
    sa_data: [u8; 14],
}

/// `struct ifmap` from `<linux/if.h>`
#[repr(C)]
#[derive(Clone, Copy)]
struct ifmap {
    _mem_start: usize,
    _mem_end: usize,
    _base_addr: c_ushort,
    _irq: u8,
    _dma: u8,
    _port: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
union ifr_ifru {
    ifru_addr: sockaddr,
    ifru_dstaddr: sockaddr,
    ifru_broadaddr: sockaddr,
    ifru_netmask: sockaddr,
    ifru_hwaddr: sockaddr,
    ifru_flags: c_ushort,
    ifru_map: ifmap,
    ifru_data: *mut c_char,
}

#[repr(C)]
struct ifreq {
    ifr_name: [c_char; IF_NAMESIZE],
    ifr_ifru: ifr_ifru,
}

impl ifreq {
    #[inline]
    fn sa_family(&self) -> c_ushort {
        // SAFETY: all the `sockaddr` union members share the same layout
        unsafe { self.ifr_ifru.ifru_addr.sa_family }
    }

    #[inline]
    fn sa_data(&self) -> [u8; 14] {
        // SAFETY: all the `sockaddr` union members share the same layout
        unsafe { self.ifr_ifru.ifru_addr.sa_data }
    }
}

#[repr(C)]
union ifc_ifcu {
    ifcu_buf: *mut c_char,
    ifcu_req: *mut ifreq,
}

#[repr(C)]
struct ifconf {
    ifc_len: c_int,
    ifc_ifcu: ifc_ifcu,
}

const _: () = assert!(mem::size_of::<ifreq>() == 40);
const _: () = assert!(mem::size_of::<ifconf>() == 16);

fn get_ifr(iface: &str) -> io::Result<ifreq> {
    if iface.len() >= IF_NAMESIZE || iface.contains('\0') {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "invalid interface name",
        ));
    }

    let mut ifr: ifreq = unsafe { mem::zeroed() };

    // The name is NUL terminated: the rest of the buffer stays zeroed
    for (dst, byte) in ifr.ifr_name.iter_mut().zip(iface.bytes()) {
        *dst = byte as c_char;
    }

    Ok(ifr)
}

#[inline]
fn get_fd() -> io::Result<OwnedFd> {
    Ok(net::socket(AddressFamily::INET, SocketType::DGRAM, None)?)
}

/// List the interfaces with an IPv4 address configured, via ioctl(SIOCGIFCONF).
/// Returns the interface names with their associated IPv4 address.
pub(super) fn list_interfaces_ioctl() -> io::Result<IndexMap<String, Ipv4Addr>> {
    let fd: OwnedFd = get_fd()?;

    // The SIOCGIFCONF ioctl does not report how much space it needs:
    // start with a small buffer and double it while it comes back full.
    let mut buf_len: usize = 8 * mem::size_of::<ifreq>();
    let mut buf: Vec<u8>;

    let ifc_len: usize = loop {
        buf = vec![0; buf_len];

        let mut ifc: ifconf = ifconf {
            ifc_len: buf_len as c_int,
            ifc_ifcu: ifc_ifcu {
                ifcu_buf: buf.as_mut_ptr() as *mut c_char,
            },
        };

        // SAFETY: the opcode matches the operand type
        unsafe { ioctl(&fd, Updater::<SIOCGIFCONF, _>::new(&mut ifc)) }
            .map_err(|err| io::Error::other(format!("SIOCGIFCONF failed: {err}")))?;

        let len: usize = ifc.ifc_len as usize;

        if len < buf_len {
            break len;
        }

        buf_len *= 2;
    };

    drop(fd);

    let reqs: *const ifreq = buf.as_ptr() as *const ifreq;
    let count: usize = ifc_len / mem::size_of::<ifreq>();

    let mut ifaces: IndexMap<String, Ipv4Addr> = IndexMap::with_capacity(count);

    for i in 0..count {
        // SAFETY: the buffer contains `count` valid `ifreq` entries
        let req: &ifreq = unsafe { &*reqs.add(i) };

        // SAFETY: ifr_name is always NUL terminated by the kernel
        let name: String = unsafe { CStr::from_ptr(req.ifr_name.as_ptr()) }
            .to_string_lossy()
            .into_owned();

        // Skip entries without an IPv4 address (e.g. IPv6-only aliases)
        if req.sa_family() != AddressFamily::INET.as_raw() {
            continue;
        }

        // ifr_addr is a sockaddr_in: sa_data is port (2 bytes, network order)
        // followed by the IPv4 address (4 bytes, network order)
        let sa_data: [u8; 14] = req.sa_data();

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

    let fd: OwnedFd = get_fd()?;

    // SAFETY: the opcode matches the operand type
    unsafe { ioctl(&fd, Updater::<SIOCGIFHWADDR, _>::new(&mut ifr)) }
        .map_err(|err| io::Error::other(format!("SIOCGIFHWADDR failed for '{iface}': {err}")))?;

    drop(fd);

    // ifr_hwaddr is a sockaddr, the first 6 bytes of sa_data are the MAC
    let sa_data: [u8; 14] = ifr.sa_data();
    let sa_family: c_ushort = ifr.sa_family();

    // Reject interfaces without an Ethernet hardware type (e.g. loopback),
    // otherwise we would silently return an all-zero MAC
    if sa_family != ARPHRD_ETHER {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("interface '{iface}' has no MAC address (hardware type {sa_family})"),
        ));
    }

    Ok(MacAddr::new([
        sa_data[0], sa_data[1], sa_data[2], sa_data[3], sa_data[4], sa_data[5],
    ]))
}

/// Read the IPv4 address for a given interface via ioctl(SIOCGIFADDR)
pub(super) fn read_ipv4(iface: &str) -> io::Result<Ipv4Addr> {
    let mut ifr: ifreq = get_ifr(iface)?;

    let fd: OwnedFd = get_fd()?;

    // SAFETY: the opcode matches the operand type
    unsafe { ioctl(&fd, Updater::<SIOCGIFADDR, _>::new(&mut ifr)) }
        .map_err(|err| io::Error::other(format!("SIOCGIFADDR failed for '{iface}': {err}")))?;

    drop(fd);

    let sa_family: c_ushort = ifr.sa_family();

    // Reject interfaces without an IPv4 address (e.g. unconfigured ones),
    // otherwise we would silently return 0.0.0.0
    if sa_family != AddressFamily::INET.as_raw() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("interface '{iface}' has no IPv4 address (sa_family {sa_family})"),
        ));
    }

    // ifr_addr is a sockaddr_in: sa_data is port (2 bytes, network order)
    // followed by the IPv4 address (4 bytes, network order)
    let sa_data: [u8; 14] = ifr.sa_data();

    Ok(Ipv4Addr::new(
        sa_data[2], sa_data[3], sa_data[4], sa_data[5],
    ))
}

/// Read the IPv4 network for a given interface via ioctl(SIOCGIFADDR)
/// and ioctl(SIOCGIFNETMASK)
pub fn read_ipv4_netmask(iface: &str) -> io::Result<Ipv4Net> {
    let mut ifr: ifreq = get_ifr(iface)?;

    let fd: OwnedFd = get_fd()?;

    // SAFETY: the opcode matches the operand type
    unsafe { ioctl(&fd, Updater::<SIOCGIFNETMASK, _>::new(&mut ifr)) }
        .map_err(|err| io::Error::other(format!("SIOCGIFNETMASK failed for '{iface}': {err}")))?;

    drop(fd);

    let sa_family: c_ushort = ifr.sa_family();

    // Reject interfaces without an IPv4 netmask (e.g. unconfigured ones)
    if sa_family != AddressFamily::INET.as_raw() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("interface '{iface}' has no IPv4 netmask (sa_family {sa_family})"),
        ));
    }

    // ifr_netmask is a sockaddr_in: sa_data is port (2 bytes, network order)
    // followed by the IPv4 netmask (4 bytes, network order)
    let sa_data: [u8; 14] = ifr.sa_data();

    let mask: Ipv4Addr = Ipv4Addr::new(sa_data[2], sa_data[3], sa_data[4], sa_data[5]);

    let prefix: u8 = mask
        .octets()
        .into_iter()
        .map(|b| b.count_ones() as u8)
        .sum();

    let ip: Ipv4Addr = read_ipv4(iface)?;

    Ipv4Net::new(ip, prefix).map_err(io::Error::other)
}
