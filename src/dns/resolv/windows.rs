use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::{io, ptr};

use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
    IP_ADAPTER_DNS_SERVER_ADDRESS_XP,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR};

pub(super) fn nameserver() -> io::Result<IpAddr> {
    const FLAGS: u32 = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;

    let mut size: u32 = 0;

    let status: u32 = unsafe {
        GetAdaptersAddresses(
            AF_UNSPEC as u32,
            FLAGS,
            ptr::null(),
            ptr::null_mut(),
            &mut size,
        )
    };

    if status != ERROR_BUFFER_OVERFLOW {
        return Err(io::Error::last_os_error());
    }

    let mut buffer: Vec<u8> = vec![0; size as usize];
    let adapters: *mut IP_ADAPTER_ADDRESSES_LH = buffer.as_mut_ptr().cast();

    let status: u32 =
        unsafe { GetAdaptersAddresses(AF_UNSPEC as u32, FLAGS, ptr::null(), adapters, &mut size) };

    if status != NO_ERROR {
        return Err(io::Error::last_os_error());
    }

    let mut adapter: *mut IP_ADAPTER_ADDRESSES_LH = adapters;

    while !adapter.is_null() {
        let mut dns: *mut IP_ADAPTER_DNS_SERVER_ADDRESS_XP =
            unsafe { (*adapter).FirstDnsServerAddress };

        while !dns.is_null() {
            let sockaddr: *mut SOCKADDR = unsafe { (*dns).Address.lpSockaddr };

            if !sockaddr.is_null() {
                if let Some(ipaddr) = parse(sockaddr) {
                    return Ok(ipaddr);
                }
            }

            dns = unsafe { (*dns).Next };
        }

        adapter = unsafe { (*adapter).Next };
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no DNS server found",
    ))
}

fn parse(sockaddr: *mut SOCKADDR) -> Option<IpAddr> {
    let raw: *const u8 = sockaddr.cast();
    let family: u16 = unsafe { raw.cast::<u16>().read() };

    if family == AF_INET {
        let octets: [u8; 4] = unsafe { raw.add(4).cast::<[u8; 4]>().read() };
        return Some(IpAddr::V4(Ipv4Addr::from(octets)));
    }

    if family == AF_INET6 {
        let octets: [u8; 16] = unsafe { raw.add(8).cast::<[u8; 16]>().read() };
        return Some(IpAddr::V6(Ipv6Addr::from(octets)));
    }

    None
}
