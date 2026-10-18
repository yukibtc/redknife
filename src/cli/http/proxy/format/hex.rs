use std::{fmt, io};

use tokio::io::{AsyncWrite, AsyncWriteExt};

use super::super::middleware::MiddlewareEvent;
use super::summary::SummaryOutput;
use crate::hexdump::HexDump;

pub(super) struct HexOutput<'a> {
    event: &'a MiddlewareEvent,
}

impl HexOutput<'_> {
    pub(super) async fn write_all<W>(&self, out: &mut W) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let output: String = self.to_string();
        out.write_all(output.as_bytes()).await
    }
}

impl<'a> From<&'a MiddlewareEvent> for HexOutput<'a> {
    fn from(event: &'a MiddlewareEvent) -> Self {
        Self { event }
    }
}

impl fmt::Display for HexOutput<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let event: &MiddlewareEvent = self.event;

        writeln!(f, "{}", SummaryOutput::from(event))?;
        writeln!(f, "ORIGINAL ({} bytes)", event.original_raw.len())?;
        write!(f, "{}", HexDump::new(&event.original_raw))?;

        if let Some(modified) = &event.modified_raw {
            writeln!(f, "MODIFIED ({} bytes)", modified.len())?;
            write!(f, "{}", HexDump::new(modified))?;
        }

        writeln!(f)
    }
}
