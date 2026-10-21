use std::io;
use std::net::IpAddr;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

/// Fetch the first DNS server configured on the system.
pub fn system_nameserver() -> io::Result<IpAddr> {
    #[cfg(unix)]
    return unix::nameserver();

    #[cfg(windows)]
    return windows::nameserver();
}
