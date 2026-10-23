use std::fs::Metadata;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::fs::File;
use tokio::io::{self, AsyncWrite, AsyncWriteExt};

/// Snapshot length, enough for standard Ethernet frames.
const SNAPLEN: u32 = 65535;

/// Link-layer type for Ethernet frames.
const LINKTYPE_ETHERNET: u32 = 1;

/// Writer producing libpcap-compatible capture streams.
pub struct PcapWriter<W> {
    writer: W,
}

impl<W> PcapWriter<W>
where
    W: AsyncWrite + Unpin,
{
    /// Creates a new capture stream over the given writer, writing the global header.
    pub async fn new(mut writer: W) -> io::Result<Self> {
        writer.write_all(&0xa1b2c3d4u32.to_le_bytes()).await?;
        writer.write_all(&2u16.to_le_bytes()).await?;
        writer.write_all(&4u16.to_le_bytes()).await?;
        writer.write_all(&0i32.to_le_bytes()).await?;
        writer.write_all(&0u32.to_le_bytes()).await?;
        writer.write_all(&SNAPLEN.to_le_bytes()).await?;
        writer.write_all(&LINKTYPE_ETHERNET.to_le_bytes()).await?;
        writer.flush().await?;

        Ok(Self { writer })
    }

    /// Writes a single packet with its per-packet header.
    pub async fn write_packet(&mut self, data: &[u8]) -> io::Result<()> {
        let ts: Duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();

        let ts_sec: u32 = ts.as_secs() as u32;
        let ts_usec: u32 = ts.subsec_micros();
        let len: u32 = data.len().min(SNAPLEN as usize) as u32;

        self.writer.write_all(&ts_sec.to_le_bytes()).await?;
        self.writer.write_all(&ts_usec.to_le_bytes()).await?;
        self.writer.write_all(&len.to_le_bytes()).await?;
        self.writer.write_all(&len.to_le_bytes()).await?;
        self.writer.write_all(&data[..len as usize]).await?;
        self.writer.flush().await?;

        Ok(())
    }
}

impl PcapWriter<File> {
    /// Opens (or creates) the capture file, writing the global header only
    /// when the file is empty, so existing captures are appended to.
    pub async fn open(path: PathBuf) -> io::Result<Self> {
        let file: File = File::options()
            .create(true)
            .write(true)
            .append(true)
            .open(path)
            .await?;

        let metadata: Metadata = file.metadata().await?;

        if metadata.len() == 0 {
            Self::new(file).await
        } else {
            Ok(Self { writer: file })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_global_header_and_packet() {
        let mut buf: Vec<u8> = Vec::new();

        {
            let mut writer: PcapWriter<&mut Vec<u8>> = PcapWriter::new(&mut buf).await.unwrap();
            writer
                .write_packet(&[0xde, 0xad, 0xbe, 0xef])
                .await
                .unwrap();
        }

        // 24 bytes global header + 16 bytes packet header + 4 bytes data
        assert_eq!(buf.len(), 24 + 16 + 4);
        assert_eq!(&buf[0..4], &0xa1b2c3d4u32.to_le_bytes());
        assert_eq!(&buf[20..24], &LINKTYPE_ETHERNET.to_le_bytes());
        assert_eq!(&buf[40..44], &[0xde, 0xad, 0xbe, 0xef]);
    }
}
