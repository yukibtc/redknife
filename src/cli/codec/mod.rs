use clap::Subcommand;

mod jwt;
mod url;

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
}

impl CodecCommand {
    pub(in crate::cli) fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Jwt { command } => command.run()?,
            Self::Url { command } => command.run()?,
        }
        Ok(())
    }
}
