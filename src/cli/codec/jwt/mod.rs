use clap::Subcommand;

mod decode;

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum JwtCommand {
    /// Decode a JWT
    #[command(arg_required_else_help = true)]
    Decode {
        /// JWT to decode
        jwt: String,
    },
}

impl JwtCommand {
    pub(super) fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Decode { jwt } => decode::run(jwt)?,
        };

        Ok(())
    }
}
