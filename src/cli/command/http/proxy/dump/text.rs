use std::io;

use bytes::Bytes;
use chrono::{DateTime, Local};
use http::Uri;
use tokio::io::{AsyncWrite, AsyncWriteExt};

use super::super::middleware::MiddlewareRequest;

pub(super) struct TextDumpOutput<'a> {
    id: u64,
    timestamp: &'a DateTime<Local>,
    uri: &'a Uri,
    raw: &'a Bytes,
}

impl<'a> From<&'a MiddlewareRequest> for TextDumpOutput<'a> {
    fn from(request: &'a MiddlewareRequest) -> Self {
        Self {
            id: request.id,
            timestamp: &request.timestamp,
            uri: &request.uri,
            raw: &request.raw,
        }
    }
}

impl TextDumpOutput<'_> {
    fn bytes(&self) -> Vec<u8> {
        let mut output: Vec<u8> = format!(
            "[{}] #{} REQUEST to {}\n",
            self.timestamp.format("%Y-%m-%d %H:%M:%S"),
            self.id,
            self.uri,
        )
        .into_bytes();

        output.extend(self.raw);

        output
    }

    pub(super) async fn write_all<W>(&self, out: &mut W) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let output: Vec<u8> = self.bytes();
        out.write_all(&output).await?;
        Ok(())
    }
}
