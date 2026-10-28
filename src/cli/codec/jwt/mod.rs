use clap::Subcommand;

mod decode;
mod encode;

use self::encode::JwtAlg;

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum JwtCommand {
    /// Decode a JWT
    #[command(arg_required_else_help = true)]
    Decode {
        /// JWT to decode
        jwt: String,
    },
    /// Encode a JWT
    #[command(arg_required_else_help = true)]
    Encode {
        /// Claims as JSON
        claims: String,
        /// JWT to decode
        #[arg(long, default_value_t = JwtAlg::HS256)]
        alg: JwtAlg,
        /// Secret for HMAC signing
        #[arg(long)]
        secret: String,
    },
}

impl JwtCommand {
    pub(super) fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Decode { jwt } => decode::run(jwt)?,
            Self::Encode {
                claims,
                alg,
                secret,
            } => encode::run(alg, claims, secret)?,
        };

        Ok(())
    }
}
