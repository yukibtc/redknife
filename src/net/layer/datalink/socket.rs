//! Raw datalink sockets (AF_PACKET).

use std::mem;
use std::num::NonZeroU32;
use std::os::fd::OwnedFd;
use std::sync::Arc;

use rustix::io::{Errno, ioctl_fionbio};
use rustix::net::addr::{SocketAddrArg, SocketAddrLen, SocketAddrOpaque};
use rustix::net::{
    self, AddressFamily, Protocol, RecvFlags, SendFlags, SocketFlags, SocketType, netdevice,
};
use tokio::io;
use tokio::io::unix::{AsyncFd, TryIoError};

const ETH_P_ALL_RAW: u16 = 0x0003;
const ARPHRD_ETHER: u16 = 1;

/// `struct sockaddr_ll` from `<linux/if_packet.h>`
///
/// ```c
/// struct sockaddr_ll {
///     unsigned short  sll_family;
///     __be16          sll_protocol;
///     int             sll_ifindex;
///     unsigned short  sll_hatype;
///     unsigned char   sll_pkttype;
///     unsigned char   sll_halen;
///     unsigned char   sll_addr[8];
/// };
/// ```
///
/// <https://github.com/torvalds/linux/blob/8066e388be48f1ad62b0449dc1d31a25489fa12a/include/uapi/linux/if_packet.h#L14>
#[repr(C)]
struct SockAddrLL {
    sll_family: u16,
    sll_protocol: u16,
    sll_ifindex: i32,
    sll_hatype: u16,
    sll_pkttype: u8,
    sll_halen: u8,
    sll_addr: [u8; 8],
}

// SAFETY: `with_sockaddr` passes a pointer to a valid, fully initialized
// `sockaddr_ll`, readable for `size_of::<SockAddrLL>()` bytes, for the whole
// duration of the call
unsafe impl SocketAddrArg for SockAddrLL {
    unsafe fn with_sockaddr<R>(
        &self,
        f: impl FnOnce(*const SocketAddrOpaque, SocketAddrLen) -> R,
    ) -> R {
        f(
            self as *const Self as *const SocketAddrOpaque,
            mem::size_of::<Self>() as SocketAddrLen,
        )
    }
}

#[derive(Clone)]
pub struct DataLinkSender {
    fd: Arc<AsyncFd<OwnedFd>>,
}

impl DataLinkSender {
    #[inline]
    fn new(fd: Arc<AsyncFd<OwnedFd>>) -> Self {
        Self { fd }
    }

    #[inline]
    pub async fn send(&self, buf: &[u8]) -> io::Result<usize> {
        send(&self.fd, buf).await
    }
}

pub struct DataLinkReceiver {
    fd: Arc<AsyncFd<OwnedFd>>,
    buf: [u8; 65536],
}

impl DataLinkReceiver {
    #[inline]
    fn new(fd: Arc<AsyncFd<OwnedFd>>) -> Self {
        Self {
            fd,
            buf: [0u8; 65536],
        }
    }

    #[inline]
    pub async fn next(&mut self) -> io::Result<Vec<u8>> {
        let n: usize = recv(&self.fd, &mut self.buf).await?;
        let frame: &[u8] = &self.buf[..n];
        Ok(frame.to_vec())
    }
}

pub struct DataLinkSocket {
    fd: AsyncFd<OwnedFd>,
}

impl DataLinkSocket {
    pub fn new(iface: &str) -> io::Result<Self> {
        let fd: OwnedFd = net::socket_with(
            AddressFamily::PACKET,
            SocketType::RAW,
            SocketFlags::CLOEXEC,
            // `new_raw_protocol`: the ethertype is stored in network order
            Some(eth_p_all()),
        )?;

        let ifindex: u32 = netdevice::name_to_index(&fd, iface)?;

        // Bind: filter packets received on this interface
        let sll: SockAddrLL = SockAddrLL {
            sll_family: AddressFamily::PACKET.as_raw(),
            sll_protocol: ETH_P_ALL_RAW.to_be(),
            sll_ifindex: ifindex as i32,
            sll_hatype: ARPHRD_ETHER,
            sll_pkttype: 0,
            sll_halen: 6,
            sll_addr: [0; 8],
        };

        net::bind(&fd, &sll)?;

        // Make the socket non-blocking
        ioctl_fionbio(&fd, true)?;

        Ok(Self {
            fd: AsyncFd::new(fd)?,
        })
    }

    #[inline]
    pub fn split(self) -> (DataLinkSender, DataLinkReceiver) {
        let fd: Arc<AsyncFd<OwnedFd>> = Arc::new(self.fd);
        (DataLinkSender::new(fd.clone()), DataLinkReceiver::new(fd))
    }
}

async fn send(fd: &AsyncFd<OwnedFd>, frame: &[u8]) -> io::Result<usize> {
    loop {
        let mut guard = fd.writable().await?;

        let res: Result<io::Result<usize>, TryIoError> =
            guard.try_io(|inner| Ok(net::send(inner, frame, SendFlags::empty())?));

        match res {
            Ok(Ok(n)) => return Ok(n),
            Ok(Err(e)) if e.raw_os_error() == Some(Errno::INTR.raw_os_error()) => continue,
            Ok(Err(e)) => return Err(e),
            Err(_would_block) => continue, // spurious wakeup: retry
        }
    }
}

async fn recv(fd: &AsyncFd<OwnedFd>, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        let mut guard = fd.readable().await?;

        let res: Result<io::Result<usize>, TryIoError> = guard.try_io(|inner| {
            let (n, _) = net::recv(inner, &mut buf[..], RecvFlags::empty())?;

            Ok(n)
        });

        match res {
            Ok(Ok(n)) => return Ok(n),
            Ok(Err(e))
                if e.raw_os_error() == Some(Errno::AGAIN.raw_os_error())
                    || e.raw_os_error() == Some(Errno::INTR.raw_os_error()) =>
            {
                continue;
            }
            Ok(Err(e)) => return Err(e),
            // Would block branch
            Err(_would_block) => continue, // spurious wakeup: ignore and retry
        }
    }
}

#[inline]
fn eth_p_all() -> Protocol {
    Protocol::from_raw(NonZeroU32::new(u32::from(ETH_P_ALL_RAW.to_be())).unwrap())
}

#[cfg(test)]
mod tests {
    use rustix::net::eth::ALL as ETH_P_ALL;

    use super::*;

    #[test]
    fn test_eth_p_all() {
        assert_eq!(eth_p_all(), ETH_P_ALL);
    }
}
