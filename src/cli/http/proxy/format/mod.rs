use std::str::FromStr;
use std::{fmt, io};

use clap::ValueEnum;
use tokio::io::AsyncWrite;

mod summary;
mod text;

use self::summary::SummaryOutput;
use self::text::TextOutput;
use super::middleware::MiddlewareEvent;

// TODO: add hex and jsonl
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(in crate::cli) enum HttpProxyFormat {
    Summary,
    Text,
}

impl fmt::Display for HttpProxyFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Summary => write!(f, "summary"),
            Self::Text => write!(f, "text"),
        }
    }
}

impl FromStr for HttpProxyFormat {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "summary" => Ok(Self::Summary),
            "text" => Ok(Self::Text),
            _ => Err("unsupported dump format"),
        }
    }
}

impl HttpProxyFormat {
    pub(super) async fn write<W>(&self, request: &MiddlewareEvent, out: &mut W) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        match self {
            Self::Summary => SummaryOutput::from(request).write_all(out).await,
            Self::Text => TextOutput::from(request).write_all(out).await,
        }
    }
}
