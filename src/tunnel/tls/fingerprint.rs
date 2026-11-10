use std::fmt;
use std::str::FromStr;

use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio_rustls::rustls::pki_types::CertificateDer;

#[derive(Debug, Error)]
#[error("invalid TLS fingerprint")]
pub struct InvalidTunnelTlsFingerprint;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TunnelTlsFingerprint([u8; 32]);

impl fmt::Display for TunnelTlsFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let encoded: String = self.to_hex();
        f.write_str(&encoded)
    }
}

impl TunnelTlsFingerprint {
    #[inline]
    pub const fn from_byte_array(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[inline]
    pub fn from_cert(cert: &CertificateDer<'_>) -> Self {
        let bytes: [u8; 32] = Sha256::digest(cert.as_ref()).into();
        Self::from_byte_array(bytes)
    }

    // #[inline]
    // fn from_slice(bytes: &[u8]) -> Result<Self, InvalidTunnelTlsFingerprint> {
    //     let bytes: [u8; 32] = bytes.try_into().map_err(|_| InvalidTunnelTlsFingerprint)?;
    //     Ok(Self(bytes))
    // }

    fn from_hex(fingerprint: &str) -> Result<Self, InvalidTunnelTlsFingerprint> {
        let mut bytes: [u8; 32] = [0; 32];

        hex::decode_to_slice(fingerprint, &mut bytes).map_err(|_| InvalidTunnelTlsFingerprint)?;

        Ok(Self::from_byte_array(bytes))
    }

    #[inline]
    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }
}

impl FromStr for TunnelTlsFingerprint {
    type Err = InvalidTunnelTlsFingerprint;

    #[inline]
    fn from_str(fingerprint: &str) -> Result<Self, Self::Err> {
        Self::from_hex(fingerprint)
    }
}
