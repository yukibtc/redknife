use std::sync::LazyLock;

use tokio_rustls::rustls;
use tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use tokio_rustls::rustls::crypto::{
    CryptoProvider, verify_tls12_signature, verify_tls13_signature,
};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{CertificateError, DigitallySignedStruct, Error, SignatureScheme};

use crate::tunnel::TunnelTlsFingerprint;

static PROVIDER: LazyLock<CryptoProvider> = LazyLock::new(rustls::crypto::ring::default_provider);

#[derive(Debug)]
pub struct TunnerlTlsVerifier {
    // Expected fingerprint, or `None` to skip verification.
    fingerprint: Option<TunnelTlsFingerprint>,
}

impl TunnerlTlsVerifier {
    /// Trust only the certificate matching fingerprint
    #[inline]
    pub fn pinned(fingerprint: TunnelTlsFingerprint) -> Self {
        Self {
            fingerprint: Some(fingerprint),
        }
    }

    /// Accept any certificate
    #[inline]
    pub fn insecure() -> Self {
        Self { fingerprint: None }
    }
}

impl From<Option<TunnelTlsFingerprint>> for TunnerlTlsVerifier {
    #[inline]
    fn from(fingerprint: Option<TunnelTlsFingerprint>) -> Self {
        match fingerprint {
            Some(fingerprint) => Self::pinned(fingerprint),
            None => Self::insecure(),
        }
    }
}

impl ServerCertVerifier for TunnerlTlsVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let Some(expected) = self.fingerprint else {
            return Ok(ServerCertVerified::assertion());
        };

        (TunnelTlsFingerprint::from_cert(end_entity) == expected)
            .then(ServerCertVerified::assertion)
            .ok_or(Error::InvalidCertificate(CertificateError::UnknownIssuer))
    }

    #[inline]
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &PROVIDER.signature_verification_algorithms,
        )
    }

    #[inline]
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &PROVIDER.signature_verification_algorithms,
        )
    }

    #[inline]
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        PROVIDER
            .signature_verification_algorithms
            .supported_schemes()
    }
}
