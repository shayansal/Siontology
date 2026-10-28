//! TLS for the mesh: TLS 1.3 only, the hybrid key exchange X25519MLKEM768
//! only, ML-DSA-44 only, mutual authentication against pinned keys.
//!
//! Nodes present their network key as a raw public key (RFC 7250): a DER
//! SubjectPublicKeyInfo with the ML-DSA-44 identifier, signed for in the
//! handshake's CertificateVerify. There are no certificates to expire and no
//! X.509 to parse. A dialing node accepts exactly the key it pinned for the
//! peer it dials; a listening node accepts exactly the keys of the peers it
//! knows, and the transport maps the key a client proved to the node it
//! belongs to. A peer that offers only classical key exchange shares no
//! group with us and fails the handshake.
//!
//! The provider is aws-lc-rs (C code; `net:aws-lc` in
//! `docs/implementation-status.md`).

use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::{AlwaysResolvesClientRawPublicKeys, Resumption};
use rustls::crypto::{aws_lc_rs, CryptoProvider, SupportedKxGroup, WebPkiSupportedAlgorithms};
use rustls::pki_types::{
    CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, SubjectPublicKeyInfoDer,
    UnixTime,
};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::AlwaysResolvesServerRawPublicKeys;
use rustls::sign::CertifiedKey;
use rustls::{
    CertificateError, DigitallySignedStruct, DistinguishedName, Error as TlsError,
    PeerIncompatible, SignatureScheme,
};

use crate::error::NetError;
use crate::key::{NetworkKey, NetworkPublicKey};

/// The application protocol the mesh speaks.
pub const ALPN: &[u8] = b"sion/1";

/// The server name a dialing node asks for. Peers are authenticated by
/// pinned keys, not names; the chain is checked in the hello that follows.
pub const SERVER_NAME: &str = "sion.invalid";

/// ML-DSA-44 is the only signature scheme the mesh accepts.
static ML_DSA_44_ONLY: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
    all: &[webpki::aws_lc_rs::ML_DSA_44],
    mapping: &[(SignatureScheme::ML_DSA_44, &[webpki::aws_lc_rs::ML_DSA_44])],
};

/// The mesh's provider: aws-lc-rs with only the TLS 1.3 suites, only the
/// hybrid group and only ML-DSA-44 signatures.
pub fn provider() -> CryptoProvider {
    provider_with(vec![aws_lc_rs::kx_group::X25519MLKEM768])
}

/// The mesh's provider with other key-exchange groups (tests use it to
/// play a peer that offers only classical key exchange).
pub fn provider_with(groups: Vec<&'static dyn SupportedKxGroup>) -> CryptoProvider {
    let base = aws_lc_rs::default_provider();
    CryptoProvider {
        cipher_suites: vec![
            aws_lc_rs::cipher_suite::TLS13_AES_128_GCM_SHA256,
            aws_lc_rs::cipher_suite::TLS13_AES_256_GCM_SHA384,
            aws_lc_rs::cipher_suite::TLS13_CHACHA20_POLY1305_SHA256,
        ],
        kx_groups: groups,
        signature_verification_algorithms: ML_DSA_44_ONLY,
        ..base
    }
}

/// The node's raw public key and the signer behind it.
fn certified_key(
    key: &NetworkKey,
    provider: &CryptoProvider,
) -> Result<Arc<CertifiedKey>, NetError> {
    let pkcs8 = key.pkcs8()?;
    let der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8.to_vec()));
    let signing = provider
        .key_provider
        .load_private_key(der)
        .map_err(|e| NetError::Tls(format!("network key: {e}")))?;
    let spki = signing
        .public_key()
        .ok_or_else(|| NetError::Tls("the network key has no public key".into()))?;
    if spki.as_ref() != key.public().spki().as_slice() {
        return Err(NetError::Tls(
            "the TLS stack derived another public key".into(),
        ));
    }
    Ok(Arc::new(CertifiedKey::new(
        vec![CertificateDer::from(spki.as_ref().to_vec())],
        signing,
    )))
}

fn verify_signature(
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, TlsError> {
    rustls::crypto::verify_tls13_signature_with_raw_key(
        message,
        &SubjectPublicKeyInfoDer::from(cert.as_ref()),
        dss,
        &ML_DSA_44_ONLY,
    )
}

/// The keys a listening node accepts, as DER SubjectPublicKeyInfo. The
/// transport keeps it in step with its peers.
pub type Allowed = Arc<RwLock<BTreeSet<Vec<u8>>>>;

/// Accepts clients whose raw public key is one of the allowed keys.
#[derive(Debug)]
struct PinnedClients {
    allowed: Allowed,
}

impl ClientCertVerifier for PinnedClients {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, TlsError> {
        let known = self
            .allowed
            .read()
            .map(|allowed| allowed.contains(end_entity.as_ref()))
            .unwrap_or(false);
        if known {
            Ok(ClientCertVerified::assertion())
        } else {
            Err(TlsError::InvalidCertificate(
                CertificateError::UnknownIssuer,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Err(TlsError::PeerIncompatible(
            PeerIncompatible::Tls13RequiredForQuic,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ML_DSA_44]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

/// Accepts exactly the server key pinned for the peer being dialed.
#[derive(Debug)]
struct PinnedServer {
    expected: Vec<u8>,
}

impl ServerCertVerifier for PinnedServer {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        if end_entity.as_ref() == self.expected.as_slice() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(TlsError::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Err(TlsError::PeerIncompatible(
            PeerIncompatible::Tls13RequiredForQuic,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ML_DSA_44]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

/// The listening side: presents `key`, accepts clients with `allowed` keys.
pub fn server_config(key: &NetworkKey, allowed: Allowed) -> Result<rustls::ServerConfig, NetError> {
    let provider = Arc::new(provider());
    let certified = certified_key(key, &provider)?;
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| NetError::Tls(e.to_string()))?
        .with_client_cert_verifier(Arc::new(PinnedClients { allowed }))
        .with_cert_resolver(Arc::new(AlwaysResolvesServerRawPublicKeys::new(certified)));
    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok(config)
}

/// The dialing side: presents `key`, accepts only a server with key
/// `expected`.
pub fn client_config(
    key: &NetworkKey,
    expected: &NetworkPublicKey,
) -> Result<rustls::ClientConfig, NetError> {
    client_config_with(key, expected, provider())
}

/// [`client_config`] with another provider (tests).
pub fn client_config_with(
    key: &NetworkKey,
    expected: &NetworkPublicKey,
    provider: CryptoProvider,
) -> Result<rustls::ClientConfig, NetError> {
    let provider = Arc::new(provider);
    let certified = certified_key(key, &provider)?;
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| NetError::Tls(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedServer {
            expected: expected.spki(),
        }))
        .with_client_cert_resolver(Arc::new(AlwaysResolvesClientRawPublicKeys::new(certified)));
    config.alpn_protocols = vec![ALPN.to_vec()];
    config.resumption = Resumption::disabled();
    Ok(config)
}

#[cfg(test)]
mod tests {
    use rustls::{ClientConnection, ServerConnection};

    use super::*;

    /// Runs a TLS handshake in memory; returns both ends, or the error that
    /// stopped it.
    fn handshake(
        client: rustls::ClientConfig,
        server: rustls::ServerConfig,
    ) -> Result<(ClientConnection, ServerConnection), TlsError> {
        let name = ServerName::try_from(SERVER_NAME).unwrap().to_owned();
        let mut client = ClientConnection::new(Arc::new(client), name)?;
        let mut server = ServerConnection::new(Arc::new(server))?;
        for _ in 0..20 {
            let mut buffer = Vec::new();
            while client.wants_write() {
                client.write_tls(&mut buffer).unwrap();
            }
            let mut reader = buffer.as_slice();
            while !reader.is_empty() {
                server.read_tls(&mut reader).unwrap();
            }
            server.process_new_packets()?;
            let mut buffer = Vec::new();
            while server.wants_write() {
                server.write_tls(&mut buffer).unwrap();
            }
            let mut reader = buffer.as_slice();
            while !reader.is_empty() {
                client.read_tls(&mut reader).unwrap();
            }
            client.process_new_packets()?;
            if !client.is_handshaking() && !server.is_handshaking() {
                return Ok((client, server));
            }
        }
        panic!("the handshake did not finish");
    }

    fn allowed(keys: &[&NetworkKey]) -> Allowed {
        Arc::new(RwLock::new(
            keys.iter().map(|k| k.public().spki()).collect(),
        ))
    }

    #[test]
    fn pinned_peers_agree_on_the_hybrid_group_and_ml_dsa_44() {
        let (server_key, client_key) = (
            NetworkKey::from_seed(&[1; 32]),
            NetworkKey::from_seed(&[2; 32]),
        );
        let (client, server) = handshake(
            client_config(&client_key, server_key.public()).unwrap(),
            server_config(&server_key, allowed(&[&client_key])).unwrap(),
        )
        .unwrap();
        for group in [
            client.negotiated_key_exchange_group().unwrap(),
            server.negotiated_key_exchange_group().unwrap(),
        ] {
            assert_eq!(group.name(), rustls::NamedGroup::X25519MLKEM768);
        }
        assert_eq!(
            client.protocol_version(),
            Some(rustls::ProtocolVersion::TLSv1_3)
        );
        assert_eq!(client.alpn_protocol(), Some(ALPN));
        // Each side saw the other's raw ML-DSA-44 key.
        assert_eq!(
            server.peer_certificates().unwrap()[0].as_ref(),
            client_key.public().spki().as_slice()
        );
        assert_eq!(
            client.peer_certificates().unwrap()[0].as_ref(),
            server_key.public().spki().as_slice()
        );
    }

    #[test]
    fn an_unknown_client_key_is_refused() {
        let server_key = NetworkKey::from_seed(&[1; 32]);
        let known = NetworkKey::from_seed(&[2; 32]);
        let stranger = NetworkKey::from_seed(&[3; 32]);
        let error = handshake(
            client_config(&stranger, server_key.public()).unwrap(),
            server_config(&server_key, allowed(&[&known])).unwrap(),
        )
        .unwrap_err();
        assert!(matches!(error, TlsError::InvalidCertificate(_)), "{error}");
    }

    #[test]
    fn a_server_with_another_key_is_refused() {
        let impostor = NetworkKey::from_seed(&[4; 32]);
        let expected = NetworkKey::from_seed(&[1; 32]);
        let client_key = NetworkKey::from_seed(&[2; 32]);
        let error = handshake(
            client_config(&client_key, expected.public()).unwrap(),
            server_config(&impostor, allowed(&[&client_key])).unwrap(),
        )
        .unwrap_err();
        assert!(matches!(error, TlsError::InvalidCertificate(_)), "{error}");
    }

    #[test]
    fn a_peer_offering_only_classical_key_exchange_fails() {
        let server_key = NetworkKey::from_seed(&[1; 32]);
        let client_key = NetworkKey::from_seed(&[2; 32]);
        let classical = provider_with(vec![
            aws_lc_rs::kx_group::X25519,
            aws_lc_rs::kx_group::SECP256R1,
        ]);
        let error = handshake(
            client_config_with(&client_key, server_key.public(), classical).unwrap(),
            server_config(&server_key, allowed(&[&client_key])).unwrap(),
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                TlsError::PeerIncompatible(_) | TlsError::AlertReceived(_)
            ),
            "{error}"
        );
    }
}
