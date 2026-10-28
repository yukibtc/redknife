use std::io;

use tokio::io::{AsyncWrite, AsyncWriteExt};

use super::super::middleware::MiddlewareEvent;
use super::super::rules::RuleOutcome;

pub(super) struct TextOutput<'a> {
    event: &'a MiddlewareEvent,
}

impl<'a> From<&'a MiddlewareEvent> for TextOutput<'a> {
    fn from(request: &'a MiddlewareEvent) -> Self {
        Self { event: request }
    }
}

impl TextOutput<'_> {
    fn bytes(&self) -> Vec<u8> {
        let event: &MiddlewareEvent = self.event;

        let outcome: String = match event.report.outcome {
            RuleOutcome::Continue => "ALLOW".to_string(),
            RuleOutcome::Deny(code) => format!("DENY {}", code.as_u16()),
        };

        let mut output: Vec<u8> = format!(
            "[{}] #{} REQUEST to {} {}",
            event.timestamp.format("%Y-%m-%d %H:%M:%S"),
            event.id,
            event.uri,
            outcome,
        )
        .into_bytes();

        if !event.report.matched_rules.is_empty() {
            output.extend_from_slice(format!(" rules={:?}", event.report.matched_rules).as_bytes());
        }

        output.extend_from_slice(
            format!("\nORIGINAL ({} bytes)\n", event.original_raw.len()).as_bytes(),
        );
        output.extend_from_slice(&event.original_raw);

        if let Some(modified_raw) = &event.modified_raw {
            output.extend_from_slice(
                format!("\n\nMODIFIED ({} bytes)\n", modified_raw.len()).as_bytes(),
            );
            output.extend_from_slice(modified_raw);
        }

        output.extend_from_slice(b"\n\n");

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
