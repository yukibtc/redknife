use std::{fmt, io};

use tokio::io::{AsyncWrite, AsyncWriteExt};

use super::super::middleware::MiddlewareEvent;
use super::super::rules::RuleOutcome;

pub(super) struct SummaryOutput<'a> {
    event: &'a MiddlewareEvent,
}

impl SummaryOutput<'_> {
    pub(super) async fn write_all<W>(&self, out: &mut W) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let output: String = format!("{self}\n");
        out.write_all(output.as_bytes()).await
    }
}

impl<'a> From<&'a MiddlewareEvent> for SummaryOutput<'a> {
    fn from(event: &'a MiddlewareEvent) -> Self {
        Self { event }
    }
}

impl fmt::Display for SummaryOutput<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let event: &MiddlewareEvent = self.event;

        write!(
            f,
            "[{}] #{} {} {} ",
            event.timestamp.format("%Y-%m-%d %H:%M:%S"),
            event.id,
            event.original.method(),
            event.uri,
        )?;

        match event.report.outcome {
            RuleOutcome::Continue => f.write_str("ALLOW")?,
            RuleOutcome::Deny(code) => {
                write!(f, "DENY {}", code.as_u16())?;
            }
        }

        match &event.modified_raw {
            Some(modified) => write!(
                f,
                " modified ({} -> {} bytes)",
                event.original_raw.len(),
                modified.len(),
            ),
            None => write!(f, " ({} bytes)", event.original_raw.len()),
        }?;

        if !event.report.matched_rules.is_empty() {
            write!(f, " rules={:?}", event.report.matched_rules)?;
        }

        Ok(())
    }
}
