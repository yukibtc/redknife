use std::io;
use std::sync::Arc;

use rcgen::{CertifiedKey, KeyPair};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio_rustls::rustls::{ClientConfig, ServerConfig};
use tokio_rustls::{TlsAcceptor, TlsConnector, rustls};

mod fingerprint;
mod verifier;

pub use self::fingerprint::TunnelTlsFingerprint;
use self::verifier::TunnerlTlsVerifier;
use super::ALPN;

#[derive(Debug)]
pub struct TunnelTlsCerts {
    cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
    fingerprint: TunnelTlsFingerprint,
}

impl TunnelTlsCerts {
    #[inline]
    pub fn new(cert: CertificateDer<'static>, key: PrivatePkcs8KeyDer<'static>) -> Self {
        Self {
            fingerprint: TunnelTlsFingerprint::from_cert(&cert),
            cert,
            key,
        }
    }

    pub fn generate() -> Self {
        let generated: CertifiedKey<KeyPair> =
            rcgen::generate_simple_self_signed(vec!["redknife".to_owned()]).unwrap();

        let cert: CertificateDer<'static> = generated.cert.der().clone();
        let key_bytes: Vec<u8> = generated.signing_key.serialize_der();
        let key: PrivatePkcs8KeyDer<'static> = PrivatePkcs8KeyDer::from(key_bytes);

        Self::new(cert, key)
    }

    #[inline]
    pub fn cert(&self) -> &CertificateDer<'static> {
        &self.cert
    }

    #[inline]
    pub fn key(&self) -> &PrivatePkcs8KeyDer<'static> {
        &self.key
    }

    #[inline]
    pub fn fingerprint(&self) -> &TunnelTlsFingerprint {
        &self.fingerprint
    }

    /// Build the TLS acceptor serving tunnel connections.
    pub(super) fn acceptor(&self) -> io::Result<TlsAcceptor> {
        let mut config: ServerConfig = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![self.cert.clone()],
                rustls::pki_types::PrivateKeyDer::Pkcs8(self.key.clone_key()),
            )
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        config.alpn_protocols = vec![ALPN.to_vec()];

        Ok(TlsAcceptor::from(Arc::new(config)))
    }
}

#[inline]
pub(super) fn connector(fingerprint: Option<TunnelTlsFingerprint>) -> io::Result<TlsConnector> {
    let verifier: TunnerlTlsVerifier = fingerprint.into();

    let mut config: ClientConfig = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    config.alpn_protocols = vec![ALPN.to_vec()];

    Ok(TlsConnector::from(Arc::new(config)))
}
