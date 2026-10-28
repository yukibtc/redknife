use std::path::PathBuf;

use clap::Subcommand;

mod jwt;
mod url;
mod xor;

use self::jwt::JwtCommand;
use self::url::UrlCommand;

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum CodecCommand {
    /// JWT utilities
    Jwt {
        #[command(subcommand)]
        command: JwtCommand,
    },
    /// URL utilities
    Url {
        #[command(subcommand)]
        command: UrlCommand,
    },
    /// XOR data with a key (XOR twice to get the original back)
    #[command(arg_required_else_help = true)]
    Xor {
        /// File to XOR, or `-` to read raw bytes from stdin
        file: PathBuf,
        /// Key from string
        #[arg(long)]
        key: Option<String>,
        /// Key from a file
        #[arg(long)]
        key_file: Option<PathBuf>,
    },
}

impl CodecCommand {
    pub(in crate::cli) fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Jwt { command } => command.run()?,
            Self::Url { command } => command.run()?,
            Self::Xor {
                file,
                key,
                key_file,
            } => xor::run(file, key, key_file)?,
        }

        Ok(())
    }
}
