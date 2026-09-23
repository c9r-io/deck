//! TLS identity: Keychain-backed key/certificate generation and decoding.
//!
//! Split out of the one-file `connector/mod.rs` on 2026-09-23; the contract
//! stays in `connector/mod.rs`.

use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Identity {
    pub(super) address: String,
    pub(super) cert_der: String,
    pub(super) key_der: String,
    pub(super) fingerprint: String,
}

impl Identity {
    pub(super) fn generate(address: &str) -> Result<Self, DeckError> {
        let ip: IpAddr = address
            .parse()
            .map_err(|_| DeckError::new(ErrorKind::Invalid, "invalid connector address"))?;
        let mut params = rcgen::CertificateParams::default();
        params.subject_alt_names = vec![rcgen::SanType::IpAddress(ip)];
        let key = rcgen::KeyPair::generate()
            .map_err(|_| DeckError::new(ErrorKind::Other, "certificate generation failed"))?;
        let cert = params
            .self_signed(&key)
            .map_err(|_| DeckError::new(ErrorKind::Other, "certificate generation failed"))?;
        let cert_der = cert.der().to_vec();
        Ok(Self {
            address: address.into(),
            fingerprint: sha(&cert_der),
            cert_der: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(cert_der),
            key_der: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.serialize_der()),
        })
    }
    pub(super) fn encode(&self) -> Result<String, DeckError> {
        Ok(format!(
            "v1_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(self)
                    .map_err(|_| DeckError::new(ErrorKind::Other, "identity encoding failed"))?
            )
        ))
    }
    pub(super) fn decode(raw: &str) -> Result<Self, DeckError> {
        let raw = raw
            .strip_prefix("v1_")
            .ok_or_else(|| DeckError::new(ErrorKind::Recovery, "connector identity is invalid"))?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(raw)
            .map_err(|_| DeckError::new(ErrorKind::Recovery, "connector identity is invalid"))?;
        serde_json::from_slice(&bytes)
            .map_err(|_| DeckError::new(ErrorKind::Recovery, "connector identity is invalid"))
    }
    pub(super) fn tls(&self) -> Result<rustls::ServerConfig, DeckError> {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
        let cert = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&self.cert_der)
            .map_err(|_| DeckError::new(ErrorKind::Recovery, "connector identity is invalid"))?;
        let key = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&self.key_der)
            .map_err(|_| DeckError::new(ErrorKind::Recovery, "connector identity is invalid"))?;
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(cert)],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)),
            )
            .map_err(|_| DeckError::new(ErrorKind::Recovery, "connector identity is invalid"))
    }
}

pub(super) fn identity_get() -> Result<Option<Identity>, DeckError> {
    keychain::get_checked(Slot::ConnectorIdentity)?
        .map(|v| Identity::decode(&v))
        .transpose()
}
