use std::io;

use bytes::Bytes;
use chrono::{DateTime, Local};
use http::{Request, Uri};
use tokio::io::{AsyncWrite, AsyncWriteExt};

use crate::http::proxy::PendingRequest;
use crate::http::request::format_request;

pub(super) struct TextDumpOutput<'a> {
    id: u64,
    timestamp: &'a DateTime<Local>,
    req: &'a Request<Bytes>,
    uri: &'a Uri,
}

impl<'a> From<&'a PendingRequest> for TextDumpOutput<'a> {
    fn from(request: &'a PendingRequest) -> Self {
        Self {
            id: request.id,
            timestamp: &request.timestamp,
            req: &request.original,
            uri: &request.original_uri,
        }
    }
}

impl TextDumpOutput<'_> {
    fn bytes(&self) -> io::Result<Vec<u8>> {
        let req: Vec<u8> = format_request(self.req).map_err(io::Error::other)?;

        let mut output: Vec<u8> = format!(
            "[{}] #{} REQUEST to {}\n",
            self.timestamp.format("%Y-%m-%d %H:%M:%S"),
            self.id,
            self.uri,
        )
        .into_bytes();

        output.extend(req);

        Ok(output)
    }

    pub(super) async fn write_all<W>(&self, out: &mut W) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let output: Vec<u8> = self.bytes()?;
        out.write_all(&output).await?;
        Ok(())
    }
}
