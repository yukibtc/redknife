//! Minimal SOCKS5 server (RFC 1928)

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio::time;

use crate::host_and_port::HostAndPort;

const VERSION: u8 = 5;
const METHOD_NO_AUTH: u8 = 0;
const METHOD_NONE: u8 = 0xFF;
const CMD_CONNECT: u8 = 1;
const ATYP_IPV4: u8 = 1;
const ATYP_DOMAIN: u8 = 3;
const ATYP_IPV6: u8 = 4;

const CODE_SUCCESS: u8 = 0x00;
const CODE_CMD_NOT_SUPPORTED: u8 = 0x07;
const CODE_ATYP_NOT_SUPPORTED: u8 = 0x08;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub struct SocksListener {
    listener: TcpListener,
}

impl SocksListener {
    #[inline]
    pub async fn bind<A>(addr: A) -> io::Result<Self>
    where
        A: ToSocketAddrs,
    {
        Ok(Self {
            listener: TcpListener::bind(addr).await?,
        })
    }

    pub async fn accept(&self) -> io::Result<(TcpStream, HostAndPort)> {
        let (mut stream, peer) = self.listener.accept().await?;

        match time::timeout(HANDSHAKE_TIMEOUT, handshake(&mut stream)).await {
            Ok(Ok(target)) => Ok((stream, target)),
            Ok(Err(err)) => {
                tracing::debug!("SOCKS handshake from {peer} failed: {err}");
                Err(err)
            }
            Err(_) => {
                tracing::debug!("SOCKS handshake from {peer} timed out");
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "SOCKS handshake timed out",
                ))
            }
        }
    }

    #[inline]
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

async fn handshake(stream: &mut TcpStream) -> io::Result<HostAndPort> {
    // Greeting: the client lists its supported methods, we pick no-auth.
    let mut greeting: [u8; 2] = [0; 2];
    stream.read_exact(&mut greeting).await?;

    if greeting[0] != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported SOCKS version {}", greeting[0]),
        ));
    }

    let nmethods: usize = greeting[1] as usize;

    let mut methods: Vec<u8> = vec![0; nmethods];
    stream.read_exact(&mut methods).await?;

    if !methods.contains(&METHOD_NO_AUTH) {
        reply_method(stream, METHOD_NONE).await?;
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "no acceptable auth method",
        ));
    }

    reply_method(stream, METHOD_NO_AUTH).await?;

    // Request: version, command, reserved, address type.
    let mut head: [u8; 4] = [0; 4];
    stream.read_exact(&mut head).await?;

    if head[0] != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported SOCKS version {}", head[0]),
        ));
    }

    if head[1] != CMD_CONNECT {
        reply_connect(stream, CODE_CMD_NOT_SUPPORTED).await?;
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "only CONNECT is supported",
        ));
    }

    let target: HostAndPort = match head[3] {
        ATYP_IPV4 => {
            let mut octets: [u8; 4] = [0; 4];
            stream.read_exact(&mut octets).await?;
            let port: u16 = read_port(stream).await?;
            HostAndPort::new(Ipv4Addr::from(octets).to_string(), port)
        }
        ATYP_DOMAIN => {
            let mut length: [u8; 1] = [0; 1];
            stream.read_exact(&mut length).await?;
            let mut domain: Vec<u8> = vec![0; length[0] as usize];
            stream.read_exact(&mut domain).await?;
            let port = read_port(stream).await?;
            HostAndPort::new(String::from_utf8_lossy(&domain).into_owned(), port)
        }
        ATYP_IPV6 => {
            let mut octets: [u8; 16] = [0; 16];
            stream.read_exact(&mut octets).await?;
            let port = read_port(stream).await?;
            HostAndPort::new(Ipv6Addr::from(octets).to_string(), port)
        }
        other => {
            reply_connect(stream, CODE_ATYP_NOT_SUPPORTED).await?;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported address type {other}"),
            ));
        }
    };

    reply_connect(stream, CODE_SUCCESS).await?;

    Ok(target)
}

#[inline]
async fn read_port(stream: &mut TcpStream) -> io::Result<u16> {
    let mut port: [u8; 2] = [0; 2];
    stream.read_exact(&mut port).await?;
    Ok(u16::from_be_bytes(port))
}

async fn reply_connect(stream: &mut TcpStream, code: u8) -> io::Result<()> {
    stream
        .write_all(&[
            VERSION, code, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ])
        .await
}

#[inline]
async fn reply_method(stream: &mut TcpStream, method: u8) -> io::Result<()> {
    stream.write_all(&[VERSION, method]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn replies_with_exact_protocol_frames() {
        let listener: SocksListener = SocksListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let _ = listener.accept().await;
        });

        let mut client: TcpStream = TcpStream::connect(addr).await.unwrap();

        // Method selection: the reply is exactly two bytes.
        client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut greeting: [u8; 2] = [0; 2];
        client.read_exact(&mut greeting).await.unwrap();
        assert_eq!(greeting, [0x05, 0x00]);

        // CONNECT towards 127.0.0.1:8080: the reply is exactly ten bytes.
        client
            .write_all(&[0x05, 0x01, 0x00, 0x01, 127, 0, 0, 1, 0x1F, 0x90])
            .await
            .unwrap();
        let mut connect_reply: [u8; 10] = [0; 10];
        client.read_exact(&mut connect_reply).await.unwrap();
        assert_eq!(
            connect_reply,
            [0x05, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        );
    }
}
