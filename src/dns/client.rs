use std::net::SocketAddr;

use bytes::Bytes;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

use super::message::{DnsMessage, DnsMessageError, DnsRecord};
use super::name::DomainName;
use super::rtype::DnsType;

pub struct DnsClient {
    socket: UdpSocket,
}

impl DnsClient {
    #[inline]
    pub fn new(socket: UdpSocket) -> Self {
        Self { socket }
    }

    pub async fn query(
        &self,
        server: SocketAddr,
        domain: DomainName,
        r#type: DnsType,
    ) -> io::Result<Vec<DnsRecord>> {
        self.socket.connect(server).await?;

        let message: DnsMessage = DnsMessage::query(domain, r#type);
        let bytes: Bytes = message.encode();

        self.socket.send(&bytes).await?;

        let mut buf: [u8; 4096] = [0; 4096];

        let len: usize = self.socket.recv(&mut buf).await?;

        match DnsMessage::decode(&buf[..len]) {
            Ok(response) => {
                if response.header.id != message.header.id {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "mismatched transaction id",
                    ));
                }

                Ok(response.answers)
            }
            Err(DnsMessageError::Truncated) => {
                self.query_tcp(server, &bytes, message.header.id).await
            }
            Err(error) => Err(io::Error::other(error)),
        }
    }

    async fn query_tcp(
        &self,
        server: SocketAddr,
        query: &[u8],
        id: u16,
    ) -> io::Result<Vec<DnsRecord>> {
        let mut stream: TcpStream = TcpStream::connect(server).await?;

        let mut packet: Vec<u8> = Vec::with_capacity(query.len() + 2);
        packet.extend_from_slice(&(query.len() as u16).to_be_bytes());
        packet.extend_from_slice(query);

        stream.write_all(&packet).await?;

        let mut length: [u8; 2] = [0; 2];
        stream.read_exact(&mut length).await?;

        let length: usize = u16::from_be_bytes(length) as usize;
        let mut buf: Vec<u8> = vec![0; length];

        stream.read_exact(&mut buf).await?;

        let response: DnsMessage = DnsMessage::decode(&buf).map_err(io::Error::other)?;

        if response.header.id != id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "mismatched transaction id",
            ));
        }

        Ok(response.answers)
    }
}
