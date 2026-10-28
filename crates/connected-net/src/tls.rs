//! TLS 1.3 with certificates identified by fingerprint instead of by a certificate authority.
//!
//! Both ends present a self-signed certificate and both verify the other's. A certificate
//! passes if its fingerprint is in the trust store, or, while pairing, unconditionally: the
//! pairing code shown to both people is what then proves who is on the other end.

use std::sync::{Arc, Mutex};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, DistinguishedName, ServerConfig,
    SignatureScheme,
};

use crate::error::Result;
use crate::fingerprint::Fingerprint;
use crate::identity::Identity;
use crate::trust::TrustStore;

/// The name clients ask for. It means nothing: certificates are matched by fingerprint.
pub(crate) const SERVER_NAME: &str = "connected.local";

#[derive(Debug, Clone)]
pub(crate) enum Mode {
    /// Only peers already in the trust store may connect.
    Trusted(Arc<Mutex<TrustStore>>),
    /// Any peer may connect, to be paired.
    Pairing,
}

#[derive(Debug)]
struct PeerVerifier {
    mode: Mode,
    algorithms: WebPkiSupportedAlgorithms,
}

impl PeerVerifier {
    fn new(mode: Mode, provider: &CryptoProvider) -> Arc<Self> {
        Arc::new(Self { mode, algorithms: provider.signature_verification_algorithms })
    }

    fn check(&self, certificate: &CertificateDer<'_>) -> std::result::Result<(), rustls::Error> {
        match &self.mode {
            Mode::Pairing => Ok(()),
            Mode::Trusted(store) => {
                let fingerprint = Fingerprint::of_certificate(certificate);
                // Looked up afresh each time, so pairing or forgetting a device in another
                // process takes effect without restarting this one.
                let trusted = store
                    .lock()
                    .map(|mut store| {
                        store.refresh();
                        store.contains(&fingerprint)
                    })
                    .unwrap_or(false);
                if trusted {
                    Ok(())
                } else {
                    Err(rustls::Error::InvalidCertificate(
                        CertificateError::ApplicationVerificationFailure,
                    ))
                }
            }
        }
    }

    fn verify_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, certificate, signed, &self.algorithms)
    }
}

impl ServerCertVerifier for PeerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        self.check(end_entity).map(|()| ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::PeerIncompatible(rustls::PeerIncompatible::Tls12NotOffered))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.verify_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

impl ClientCertVerifier for PeerVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        self.check(end_entity).map(|()| ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::PeerIncompatible(rustls::PeerIncompatible::Tls12NotOffered))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.verify_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

pub(crate) fn server_config(identity: &Identity, mode: Mode) -> Result<Arc<ServerConfig>> {
    let provider = provider();
    let verifier = PeerVerifier::new(mode, &provider);
    let config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(verifier)
        .with_single_cert(identity.cert_chain(), identity.private_key())?;
    Ok(Arc::new(config))
}

pub(crate) fn client_config(identity: &Identity, mode: Mode) -> Result<Arc<ClientConfig>> {
    let provider = provider();
    let verifier = PeerVerifier::new(mode, &provider);
    let config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(identity.cert_chain(), identity.private_key())?;
    Ok(Arc::new(config))
}
