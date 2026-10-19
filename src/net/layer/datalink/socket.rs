use std::ffi::{CString, c_int, c_uint};
use std::fs::File;
use std::io::ErrorKind;
use std::mem;
use std::os::fd::AsRawFd;
use std::os::unix::io::{FromRawFd, RawFd};
use std::sync::Arc;

use libc::{AF_PACKET, ETH_P_ALL, SOCK_CLOEXEC, SOCK_RAW, ssize_t};
use tokio::io;
use tokio::io::unix::{AsyncFd, TryIoError};

struct FdGuard(RawFd);

impl Drop for FdGuard {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

#[derive(Clone)]
pub struct DataLinkSender {
    file: Arc<AsyncFd<File>>,
}

impl DataLinkSender {
    #[inline]
    fn new(file: Arc<AsyncFd<File>>) -> Self {
        Self { file }
    }

    #[inline]
    pub async fn send(&self, buf: &[u8]) -> io::Result<usize> {
        send(&self.file, buf).await
    }
}

pub struct DataLinkReceiver {
    file: Arc<AsyncFd<File>>,
    buf: [u8; 65536],
}

impl DataLinkReceiver {
    #[inline]
    fn new(file: Arc<AsyncFd<File>>) -> Self {
        Self {
            file,
            buf: [0u8; 65536],
        }
    }

    #[inline]
    pub async fn next(&mut self) -> io::Result<Vec<u8>> {
        let n: usize = recv(&self.file, &mut self.buf).await?;
        let frame: &[u8] = &self.buf[..n];
        Ok(frame.to_vec())
    }
}

pub struct DataLinkSocket {
    file: AsyncFd<File>,
}

impl DataLinkSocket {
    pub fn new(iface: &str) -> io::Result<Self> {
        let fd: c_int = unsafe {
            libc::socket(
                AF_PACKET,
                SOCK_RAW | SOCK_CLOEXEC,
                (ETH_P_ALL as u16).to_be() as c_int,
            )
        };

        if fd < 0 {
            let err: String = format!("socket(AF_PACKET) failed: {}", io::Error::last_os_error());
            return Err(io::Error::other(err));
        }

        let guard = FdGuard(fd);

        let ifindex: c_uint = unsafe { libc::if_nametoindex(CString::new(iface)?.as_ptr()) };

        if ifindex == 0 {
            return Err(io::Error::new(ErrorKind::NotFound, "interface not found"));
        }

        // Bind: filter packets received on this interface
        let sll = libc::sockaddr_ll {
            sll_family: AF_PACKET as u16,
            sll_protocol: (ETH_P_ALL as u16).to_be(),
            sll_ifindex: ifindex as i32,
            sll_hatype: 1, // ARPHRD_ETHER
            sll_pkttype: 0,
            sll_halen: 6,
            sll_addr: [0; 8],
        };

        let bind: c_int = unsafe {
            libc::bind(
                fd,
                &sll as *const libc::sockaddr_ll as *const libc::sockaddr,
                size_of::<libc::sockaddr_ll>() as u32,
            )
        };

        if bind < 0 {
            let err: String = format!("bind(AF_PACKET) failed: {}", io::Error::last_os_error());
            return Err(io::Error::other(err));
        }

        // Extract the fd and make it non-blocking
        let raw_fd: RawFd = unsafe {
            let fd: RawFd = guard.0;
            mem::forget(guard); // transfers ownership of the fd
            let flags: c_int = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            fd
        };

        let file: File = unsafe { File::from_raw_fd(raw_fd) };

        Ok(Self {
            file: AsyncFd::new(file)?,
        })
    }

    #[inline]
    pub fn split(self) -> (DataLinkSender, DataLinkReceiver) {
        let file: Arc<AsyncFd<File>> = Arc::new(self.file);
        (
            DataLinkSender::new(file.clone()),
            DataLinkReceiver::new(file),
        )
    }
}

async fn send(file: &AsyncFd<File>, frame: &[u8]) -> io::Result<usize> {
    loop {
        let mut guard = file.writable().await?;

        let res: Result<io::Result<usize>, TryIoError> = guard.try_io(|inner| unsafe {
            let n: ssize_t = libc::send(
                inner.as_raw_fd(),
                frame.as_ptr() as *const libc::c_void,
                frame.len(),
                0,
            );

            if n < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(n as usize)
            }
        });

        match res {
            Ok(Ok(n)) => return Ok(n),
            Ok(Err(e)) if matches!(e.raw_os_error(), Some(libc::EINTR)) => continue,
            Ok(Err(e)) => return Err(e),
            Err(_would_block) => continue, // spurious wakeup: retry
        }
    }
}

async fn recv(file: &AsyncFd<File>, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        let mut guard = file.readable().await?;

        let res: Result<io::Result<usize>, TryIoError> = guard.try_io(|inner| unsafe {
            let n: isize = libc::recv(
                inner.as_raw_fd(),
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
                0,
            );

            if n < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(n as usize)
            }
        });

        match res {
            Ok(Ok(n)) => {
                if n == 0 {
                    continue;
                }

                return Ok(n);
            }
            Ok(Err(e)) if matches!(e.raw_os_error(), Some(libc::EAGAIN) | Some(libc::EINTR)) => {
                continue;
            }
            Ok(Err(e)) => return Err(e),
            // Would block branch
            Err(_would_block) => continue, // spurious wakeup: ignore and retry
        }
    }
}
