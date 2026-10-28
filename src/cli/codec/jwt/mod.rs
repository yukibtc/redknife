use std::io::{self, Read};

use clap::Subcommand;

mod decode;
mod encode;

use self::encode::JwtAlg;

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum JwtCommand {
    /// Decode a JWT
    #[command(arg_required_else_help = true)]
    Decode {
        /// JWT to decode, or `-` to read from stdin
        jwt: String,
    },
    /// Encode a JWT
    #[command(arg_required_else_help = true)]
    Encode {
        /// Claims as JSON, or `-` to read from stdin
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
            Self::Decode { jwt } => {
                let jwt: String = str_from_arg_or_stdin(jwt)?;
                decode::run(jwt)?
            }
            Self::Encode {
                claims,
                alg,
                secret,
            } => {
                let claims: String = str_from_arg_or_stdin(claims)?;
                encode::run(alg, claims, secret)?
            }
        };

        Ok(())
    }
}

fn str_from_arg_or_stdin(arg: String) -> io::Result<String> {
    match arg.as_str() {
        "-" => {
            let mut buffer = String::new();
            io::stdin().read_to_string(&mut buffer)?;
            Ok(buffer)
        }
        _ => Ok(arg),
    }
}
