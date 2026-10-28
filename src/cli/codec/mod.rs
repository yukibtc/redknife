use clap::Subcommand;

use crate::cli::codec::jwt::JwtCommand;

mod jwt;

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum CodecCommand {
    /// JWT utilities
    Jwt {
        #[command(subcommand)]
        command: JwtCommand,
    },
}

impl CodecCommand {
    pub(in crate::cli) fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Jwt { command } => command.run()?,
        }
        Ok(())
    }
}
