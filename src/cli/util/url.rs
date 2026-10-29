use clap::Subcommand;
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

use crate::cli::str_from_arg_or_stdin;

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum UrlCommand {
    /// URL decode
    #[command(arg_required_else_help = true)]
    Decode {
        /// URL to decode, or `-` to read from stdin
        url: String,
    },
    /// URL encode
    #[command(arg_required_else_help = true)]
    Encode {
        /// URL to encode, or `-` to read from stdin
        url: String,
    },
}

impl UrlCommand {
    pub(super) fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Decode { url } => run(url, true),
            Self::Encode { url } => run(url, false),
        }
    }
}

pub(super) fn run(url: String, decode: bool) -> Result<(), Box<dyn std::error::Error>> {
    let url: String = str_from_arg_or_stdin(url)?;

    if decode {
        let decoded = percent_decode_str(&url).decode_utf8()?;
        println!("{decoded}");
    } else {
        println!("{}", utf8_percent_encode(url.trim(), NON_ALPHANUMERIC));
    }

    Ok(())
}
