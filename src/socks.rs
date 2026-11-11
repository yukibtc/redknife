//! Minimal SOCKS5 client and server (RFC 1928)

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

/// Connect to `target` through a SOCKS5 proxy (CONNECT, no authentication).
pub async fn connect(proxy: SocketAddr, target: &HostAndPort) -> io::Result<TcpStream> {
    let mut stream: TcpStream = time::timeout(HANDSHAKE_TIMEOUT, TcpStream::connect(proxy))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "SOCKS proxy connect timed out"))??;

    match time::timeout(HANDSHAKE_TIMEOUT, connect_handshake(&mut stream, target)).await {
        Ok(result) => result.map(|()| stream),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "SOCKS handshake timed out",
        )),
    }
}

// Run the client side of the SOCKS5 CONNECT handshake.
async fn connect_handshake(stream: &mut TcpStream, target: &HostAndPort) -> io::Result<()> {
    // Greeting: offer the no-auth method only.
    stream.write_all(&[VERSION, 0x01, METHOD_NO_AUTH]).await?;

    let mut method: [u8; 2] = [0; 2];
    stream.read_exact(&mut method).await?;

    if method[0] != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported SOCKS version {}", method[0]),
        ));
    }

    if method[1] != METHOD_NO_AUTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the proxy selected an unsupported auth method",
        ));
    }

    // Request: CONNECT, with the target encoded per its address type.
    let mut request: Vec<u8> = vec![VERSION, CMD_CONNECT, 0x00];

    match target.host().parse::<Ipv4Addr>() {
        Ok(ip) => {
            request.push(ATYP_IPV4);
            request.extend_from_slice(&ip.octets());
        }
        Err(_) => match target.host().parse::<Ipv6Addr>() {
            Ok(ip) => {
                request.push(ATYP_IPV6);
                request.extend_from_slice(&ip.octets());
            }
            Err(_) => {
                request.push(ATYP_DOMAIN);
                request.push(u8::try_from(target.host().len()).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidInput, "target hostname too long")
                })?);
                request.extend_from_slice(target.host().as_bytes());
            }
        },
    }
    request.extend_from_slice(&target.port().to_be_bytes());

    stream.write_all(&request).await?;

    // Reply: version, code, reserved, bound address type and bound address.
    let mut reply: [u8; 4] = [0; 4];
    stream.read_exact(&mut reply).await?;

    if reply[0] != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported SOCKS version {}", reply[0]),
        ));
    }

    if reply[1] != CODE_SUCCESS {
        return Err(io::Error::other(format!(
            "SOCKS proxy refused CONNECT ({:#04x})",
            reply[1]
        )));
    }

    // Consume the bound address, whose encoding depends on the address type.
    match reply[3] {
        ATYP_IPV4 => {
            let mut bound: [u8; 6] = [0; 6];
            stream.read_exact(&mut bound).await?;
        }
        ATYP_DOMAIN => {
            let mut length: [u8; 1] = [0; 1];
            stream.read_exact(&mut length).await?;

            let mut bound: Vec<u8> = vec![0; usize::from(length[0]) + 2];
            stream.read_exact(&mut bound).await?;
        }
        ATYP_IPV6 => {
            let mut bound: [u8; 18] = [0; 18];
            stream.read_exact(&mut bound).await?;
        }
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported reply address type {other}"),
            ));
        }
    }

    Ok(())
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

    #[tokio::test]
    async fn connect_relays_through_a_socks_listener() {
        // The origin server ultimately reached through the proxy.
        let origin: TcpListener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr: SocketAddr = origin.local_addr().unwrap();
        tokio::spawn(async move {
            let (socket, _) = origin.accept().await.unwrap();
            let (mut read, mut write) = socket.into_split();
            let _ = tokio::io::copy(&mut read, &mut write).await;
        });

        // The SOCKS listener plays the proxy: accepted connections are relayed
        // to the origin.
        let listener: SocksListener = SocksListener::bind("127.0.0.1:0").await.unwrap();
        let proxy: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, target) = listener.accept().await.unwrap();
            assert_eq!(target, HostAndPort::from(origin_addr));

            let mut upstream: TcpStream = TcpStream::connect((target.host(), target.port()))
                .await
                .unwrap();
            let mut stream = stream;
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
        });

        let mut client: TcpStream = super::connect(proxy, &HostAndPort::from(origin_addr))
            .await
            .unwrap();

        client.write_all(b"ping").await.unwrap();
        let mut buffer: [u8; 4] = [0; 4];
        client.read_exact(&mut buffer).await.unwrap();
        assert_eq!(&buffer, b"ping");

        client.write_all(b"pongpong").await.unwrap();
        let mut buffer: [u8; 8] = [0; 8];
        client.read_exact(&mut buffer).await.unwrap();
        assert_eq!(&buffer, b"pongpong");
    }
}
