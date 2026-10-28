use std::fmt;
use std::str::FromStr;

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy)]
pub(in crate::cli) enum JwtAlg {
    HS256,
    HS384,
    HS512,
    RS256,
    RS384,
    RS512,
    PS256,
    PS384,
    PS512,
    ES256,
    ES384,
    EdDSA,
}

impl fmt::Display for JwtAlg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HS256 => f.write_str("HS256"),
            Self::HS384 => f.write_str("HS384"),
            Self::HS512 => f.write_str("HS512"),
            Self::RS256 => f.write_str("RS256"),
            Self::RS384 => f.write_str("RS384"),
            Self::RS512 => f.write_str("RS512"),
            Self::PS256 => f.write_str("PS256"),
            Self::PS384 => f.write_str("PS384"),
            Self::PS512 => f.write_str("PS512"),
            Self::ES256 => f.write_str("ES256"),
            Self::ES384 => f.write_str("ES384"),
            Self::EdDSA => f.write_str("EdDSA"),
        }
    }
}

impl FromStr for JwtAlg {
    type Err = &'static str;

    fn from_str(alg: &str) -> Result<Self, Self::Err> {
        match alg {
            "HS256" => Ok(Self::HS256),
            "HS384" => Ok(Self::HS384),
            "HS512" => Ok(Self::HS512),
            "RS256" => Ok(Self::RS256),
            "RS384" => Ok(Self::RS384),
            "RS512" => Ok(Self::RS512),
            "PS256" => Ok(Self::PS256),
            "PS384" => Ok(Self::PS384),
            "PS512" => Ok(Self::PS512),
            "ES256" => Ok(Self::ES256),
            "ES384" => Ok(Self::ES384),
            "EdDSA" => Ok(Self::EdDSA),
            _ => Err("Unsupported JWT algorithm"),
        }
    }
}

impl From<JwtAlg> for Algorithm {
    fn from(alg: JwtAlg) -> Self {
        match alg {
            JwtAlg::HS256 => Self::HS256,
            JwtAlg::HS384 => Self::HS384,
            JwtAlg::HS512 => Self::HS512,
            JwtAlg::RS256 => Self::RS256,
            JwtAlg::RS384 => Self::RS384,
            JwtAlg::RS512 => Self::RS512,
            JwtAlg::PS256 => Self::PS256,
            JwtAlg::PS384 => Self::PS384,
            JwtAlg::PS512 => Self::PS512,
            JwtAlg::ES256 => Self::ES256,
            JwtAlg::ES384 => Self::ES384,
            JwtAlg::EdDSA => Self::EdDSA,
        }
    }
}

pub(super) fn run(
    alg: JwtAlg,
    claims: String,
    secret: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let algorithm: Algorithm = alg.into();
    let header: Header = Header::new(algorithm);

    let claims: Map<String, Value> = serde_json::from_str(&claims)?;

    let secret: EncodingKey = EncodingKey::from_secret(secret.as_bytes());

    let token: String = jsonwebtoken::encode(&header, &claims, &secret)?;

    println!("{token}");

    Ok(())
}
