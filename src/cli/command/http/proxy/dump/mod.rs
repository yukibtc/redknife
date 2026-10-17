use std::str::FromStr;
use std::{fmt, io};

use clap::ValueEnum;
use tokio::io::AsyncWrite;

mod text;

use self::text::TextDumpOutput;
use crate::http::proxy::PendingRequest;

// TODO: add hex and jsonl
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(in crate::cli) enum HttpProxyDumpFormat {
    Text,
}

impl fmt::Display for HttpProxyDumpFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text => write!(f, "text"),
        }
    }
}

impl FromStr for HttpProxyDumpFormat {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "text" => Ok(Self::Text),
            _ => Err("unsupported dump format"),
        }
    }
}

impl HttpProxyDumpFormat {
    pub(super) async fn write<W>(&self, request: &PendingRequest, out: &mut W) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        match self {
            Self::Text => TextDumpOutput::from(request).write_all(out).await,
        }
    }
}
