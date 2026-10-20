use std::fs::Metadata;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::fs::File;
use tokio::io::{self, AsyncWriteExt};

/// Snapshot length, enough for standard Ethernet frames.
const SNAPLEN: u32 = 65535;

/// Link-layer type for Ethernet frames.
const LINKTYPE_ETHERNET: u32 = 1;

/// Writer producing libpcap-compatible capture files.
pub struct PcapWriter {
    file: File,
}

impl PcapWriter {
    /// Opens (or creates) the capture file, writing the global header only
    /// when the file is empty, so existing captures are appended to.
    pub async fn open(path: PathBuf) -> io::Result<Self> {
        let file: File = File::options()
            .create(true)
            .write(true)
            .append(true)
            .open(path)
            .await?;

        let mut writer: Self = Self { file };

        let metadata: Metadata = writer.file.metadata().await?;

        if metadata.len() == 0 {
            writer.write_global_header().await?;
        }

        Ok(writer)
    }

    /// Writes a single packet with its per-packet header.
    pub async fn write_packet(&mut self, data: &[u8]) -> io::Result<()> {
        let ts: Duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();

        let ts_sec: u32 = ts.as_secs() as u32;
        let ts_usec: u32 = ts.subsec_micros();
        let len: u32 = data.len().min(SNAPLEN as usize) as u32;

        self.file.write_all(&ts_sec.to_le_bytes()).await?;
        self.file.write_all(&ts_usec.to_le_bytes()).await?;
        self.file.write_all(&len.to_le_bytes()).await?;
        self.file.write_all(&len.to_le_bytes()).await?;
        self.file.write_all(&data[..len as usize]).await?;
        self.file.flush().await?;

        Ok(())
    }

    async fn write_global_header(&mut self) -> io::Result<()> {
        self.file.write_all(&0xa1b2c3d4u32.to_le_bytes()).await?;
        self.file.write_all(&2u16.to_le_bytes()).await?;
        self.file.write_all(&4u16.to_le_bytes()).await?;
        self.file.write_all(&0i32.to_le_bytes()).await?;
        self.file.write_all(&0u32.to_le_bytes()).await?;
        self.file.write_all(&SNAPLEN.to_le_bytes()).await?;
        self.file
            .write_all(&LINKTYPE_ETHERNET.to_le_bytes())
            .await?;
        self.file.flush().await?;

        Ok(())
    }
}
