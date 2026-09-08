//! TLS 1.3 for the LAN TCP transport, on the identity the QUIC path already uses.
//!
//! The blob protocol runs over any bidirectional stream pair (see
//! `examples/blob_over_tcp.rs`, which measured 3.9x on a phone whose kernel has
//! no UDP GSO). What a TCP socket does *not* bring is iroh's authentication and
//! encryption, and this module supplies exactly that and nothing more.
//!
//! # The scheme, and why it is a copy
//!
//! This mirrors `iroh`'s own TLS: **TLS 1.3 only**, peers identified by
//! **RFC 7250 raw public keys** carrying the endpoint's ed25519 key, and
//! **mutual authentication** — each side presents a key and verifies the
//! other's signature. Reimplemented rather than reused because iroh keeps
//! `make_client_config`/`make_server_config` `pub(crate)` and they return
//! `Quic*Config`; the crypto provider is taken from
//! [`iroh::tls::default_provider`] so at least that is shared and cannot drift.
//!
//! Copying a reviewed, deployed scheme is the point. The alternative — a
//! self-signed X.509 pinned by fingerprint, which is what LocalSend does —
//! would introduce a second notion of identity alongside the `EndpointId` Wisp
//! already has, backs up, and restores.
//!
//! # What a leaked identity key does and does not allow
//!
//! The ed25519 key **only signs** the handshake transcript. TLS 1.3 always
//! derives session keys from an ephemeral (E)CDHE exchange, so an attacker who
//! later obtains the key **cannot decrypt traffic they recorded earlier**. What
//! they can do is impersonate the device on new connections — and that is
//! already true of the QUIC path, which authenticates with the very same key.
//! This transport therefore adds no exposure that iroh did not already have.
//!
//! For that to hold, the identity key must never be converted to X25519 and
//! used for *static* key agreement: that would make one key leak decrypt every
//! recorded session, and reusing one key across EdDSA and X25519 is its own
//! hazard. Signing only.
//!
//! Session resumption and 0-RTT are **disabled**. A resumption PSK weakens
//! forward secrecy and 0-RTT data is replayable, and on a LAN — 6 ms round trip
//! in the measurements — a full handshake costs nothing worth that.

use std::sync::Arc;

use iroh::{PublicKey, SecretKey};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{WebPkiSupportedAlgorithms, verify_tls13_signature_with_raw_key};
use rustls::pki_types::{
    CertificateDer, InvalidSignature, SignatureVerificationAlgorithm, SubjectPublicKeyInfoDer,
    UnixTime, alg_id,
};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, DistinguishedName, ServerConfig,
    SignatureScheme, SupportedProtocolVersion,
};

use crate::blobs::error::{BlobError, BlobTextError, Result};

/// The only TLS version this transport speaks.
const PROTOCOL_VERSIONS: &[&SupportedProtocolVersion] = &[&rustls::version::TLS13];

const ED25519: Ed25519Identity = Ed25519Identity;

/// ed25519 is the only signature scheme offered or accepted, because the
/// identity being authenticated is an ed25519 endpoint key.
const SUPPORTED_SIG_ALGS: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
    all: &[&ED25519],
    mapping: &[(SignatureScheme::ED25519, &[&ED25519])],
};

/// Reads the peer's identity out of the certificates rustls verified.
///
/// The handshake proves the peer holds the secret half of whatever key it
/// presented; this says *which* key that was. Both sides need it — the server
/// to decide whether to accept the sender at all, the client to confirm it
/// reached the peer it meant to — and it is the equivalent of iroh's
/// `Connection::remote_id`.
///
/// Errors if there is no certificate, if more than one was sent, or if the one
/// sent is not an ed25519 raw public key.
pub fn peer_identity(certificates: Option<&[CertificateDer<'_>]>) -> Result<PublicKey> {
    let fail = |what: &str| BlobError::connect("lan tls peer".to_owned(), BlobTextError::new(what));
    let certificates = certificates.ok_or_else(|| fail("peer presented no certificate"))?;
    let [certificate] = certificates else {
        return Err(fail("peer presented a certificate chain, not a raw key"));
    };
    // A raw public key "certificate" is a SPKI: a fixed 12-byte ed25519 prefix
    // followed by the 32 key bytes. Rebuilding the SPKI from the tail and
    // comparing is what checks the prefix, so a wrong algorithm cannot slip
    // through as a truncated key.
    let spki = certificate.as_ref();
    let key_bytes: [u8; 32] = spki
        .get(spki.len().saturating_sub(32)..)
        .and_then(|tail| tail.try_into().ok())
        .ok_or_else(|| fail("peer key is too short to be ed25519"))?;
    let key = PublicKey::from_bytes(&key_bytes).map_err(|_| fail("peer key is not ed25519"))?;
    if rustls::sign::public_key_to_spki(&alg_id::ED25519, key.as_bytes()).as_ref() != spki {
        return Err(fail("peer key is not an ed25519 raw public key"));
    }
    Ok(key)
}

/// Client side: present `secret`'s identity, and accept **only** `expected_peer`.
///
/// The expected key is held in the verifier rather than smuggled through the
/// SNI name the way iroh does it. iroh has to do that because quinn's API only
/// hands the verifier a server name; here the caller builds the config, so the
/// key can be passed directly and there is no name codec to get wrong. The
/// `ServerName` the connector still requires is therefore not load-bearing and
/// must not be treated as if it were.
pub fn client_config(secret: &SecretKey, expected_peer: PublicKey) -> Result<Arc<ClientConfig>> {
    let mut config = ClientConfig::builder_with_provider(iroh::tls::default_provider())
        .with_protocol_versions(PROTOCOL_VERSIONS)
        .map_err(|source| tls_setup("selecting TLS 1.3 for the client", source))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedPeerVerifier { expected_peer }))
        .with_client_cert_resolver(Arc::new(RawPublicKeyCert::new(secret)));
    // See the module docs: no resumption, no 0-RTT.
    config.resumption = rustls::client::Resumption::disabled();
    config.enable_early_data = false;
    Ok(Arc::new(config))
}

/// Server side: present `secret`'s identity and require a raw public key from
/// the client, whose value the caller reads back with [`peer_identity`].
///
/// Accepting any well-formed key here is deliberate and matches iroh: the
/// handshake's job is to prove possession, and *authorisation* — is this sender
/// paired, is this transfer expected — is the application's decision, made once
/// against an identity rather than twice against two different notions of one.
pub fn server_config(secret: &SecretKey) -> Result<Arc<ServerConfig>> {
    let mut config = ServerConfig::builder_with_provider(iroh::tls::default_provider())
        .with_protocol_versions(PROTOCOL_VERSIONS)
        .map_err(|source| tls_setup("selecting TLS 1.3 for the server", source))?
        .with_client_cert_verifier(Arc::new(AnyRawPublicKeyVerifier))
        .with_cert_resolver(Arc::new(RawPublicKeyCert::new(secret)));
    // Issue no resumption tickets, so a session cannot be resumed later.
    config.send_tls13_tickets = 0;
    Ok(Arc::new(config))
}

fn tls_setup(context: &str, source: rustls::Error) -> BlobError {
    BlobError::connect(
        context.to_owned(),
        BlobTextError::new(format!("{source:#}")),
    )
}

// --- certificate presentation ---------------------------------------------

/// Presents the endpoint's ed25519 key as an RFC 7250 raw public key, and signs
/// the handshake with it. Serves both roles, as iroh's equivalent does.
#[derive(Debug)]
struct RawPublicKeyCert {
    key: Arc<rustls::sign::CertifiedKey>,
}

impl RawPublicKeyCert {
    fn new(secret: &SecretKey) -> Self {
        let signing_key = Arc::new(IdentityKey(secret.clone()));
        let spki = signing_key.spki();
        let as_certificate = CertificateDer::from(spki.as_ref().to_vec());
        Self {
            key: Arc::new(rustls::sign::CertifiedKey::new(
                vec![as_certificate],
                signing_key,
            )),
        }
    }
}

impl rustls::client::ResolvesClientCert for RawPublicKeyCert {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        _sigschemes: &[SignatureScheme],
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(Arc::clone(&self.key))
    }

    fn only_raw_public_keys(&self) -> bool {
        true
    }

    fn has_certs(&self) -> bool {
        true
    }
}

impl rustls::server::ResolvesServerCert for RawPublicKeyCert {
    fn resolve(
        &self,
        _client_hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(Arc::clone(&self.key))
    }

    fn only_raw_public_keys(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone)]
struct IdentityKey(SecretKey);

impl IdentityKey {
    fn spki(&self) -> SubjectPublicKeyInfoDer<'static> {
        rustls::sign::public_key_to_spki(&alg_id::ED25519, self.0.public().as_bytes())
    }
}

impl rustls::sign::SigningKey for IdentityKey {
    fn choose_scheme(&self, offered: &[SignatureScheme]) -> Option<Box<dyn rustls::sign::Signer>> {
        offered
            .contains(&SignatureScheme::ED25519)
            .then(|| Box::new(self.clone()) as Box<dyn rustls::sign::Signer>)
    }

    fn algorithm(&self) -> rustls::SignatureAlgorithm {
        rustls::SignatureAlgorithm::ED25519
    }

    fn public_key(&self) -> Option<SubjectPublicKeyInfoDer<'_>> {
        Some(self.spki())
    }
}

impl rustls::sign::Signer for IdentityKey {
    fn sign(&self, message: &[u8]) -> std::result::Result<Vec<u8>, rustls::Error> {
        Ok(self.0.sign(message).to_bytes().to_vec())
    }

    fn scheme(&self) -> SignatureScheme {
        SignatureScheme::ED25519
    }
}

// --- verification ----------------------------------------------------------

/// Accepts one peer and no other.
#[derive(Debug)]
struct PinnedPeerVerifier {
    expected_peer: PublicKey,
}

impl ServerCertVerifier for PinnedPeerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        if !intermediates.is_empty() {
            return Err(rustls::Error::InvalidCertificate(
                CertificateError::UnknownIssuer,
            ));
        }
        // Compare the whole SPKI, not just the key bytes: that also pins the
        // algorithm identifier, so a key of another type cannot match by having
        // the same tail. The server name is ignored on purpose — the pinned key
        // is the identity here, and treating a name as a second check would
        // imply a guarantee nothing issues.
        let expected =
            rustls::sign::public_key_to_spki(&alg_id::ED25519, self.expected_peer.as_bytes());
        if expected.as_ref() != end_entity.as_ref() {
            return Err(rustls::Error::InvalidCertificate(
                CertificateError::NotValidForName,
            ));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::PeerIncompatible(
            rustls::PeerIncompatible::Tls12NotOffered,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature_with_raw_key(
            message,
            &SubjectPublicKeyInfoDer::from(cert.as_ref()),
            dss,
            &SUPPORTED_SIG_ALGS,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        SUPPORTED_SIG_ALGS.supported_schemes()
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

/// Requires the client to present a raw public key and prove it holds the
/// secret half, without deciding whether that identity is welcome.
#[derive(Debug)]
struct AnyRawPublicKeyVerifier;

impl ClientCertVerifier for AnyRawPublicKeyVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        if !intermediates.is_empty() {
            return Err(rustls::Error::InvalidCertificate(
                CertificateError::UnknownIssuer,
            ));
        }
        // Reject anything that is not an ed25519 raw public key here rather
        // than leaving it to the caller: a malformed key would otherwise reach
        // `peer_identity` only after the handshake had already succeeded.
        peer_identity(Some(std::slice::from_ref(end_entity)))
            .map_err(|_| rustls::Error::InvalidCertificate(CertificateError::BadEncoding))?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::PeerIncompatible(
            rustls::PeerIncompatible::Tls12NotOffered,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature_with_raw_key(
            message,
            &SubjectPublicKeyInfoDer::from(cert.as_ref()),
            dss,
            &SUPPORTED_SIG_ALGS,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        SUPPORTED_SIG_ALGS.supported_schemes()
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

/// ed25519 verification, wired into rustls' raw-public-key path.
#[derive(Debug)]
struct Ed25519Identity;

impl SignatureVerificationAlgorithm for Ed25519Identity {
    fn verify_signature(
        &self,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> std::result::Result<(), InvalidSignature> {
        let public_key = PublicKey::try_from(public_key).map_err(|_| InvalidSignature)?;
        let signature = iroh::Signature::try_from(signature).map_err(|_| InvalidSignature)?;
        public_key
            .verify(message, &signature)
            .map_err(|_| InvalidSignature)
    }

    fn public_key_alg_id(&self) -> rustls::pki_types::AlgorithmIdentifier {
        alg_id::ED25519
    }

    fn signature_alg_id(&self) -> rustls::pki_types::AlgorithmIdentifier {
        alg_id::ED25519
    }

    fn fips(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::ServerName;
    use tokio::net::{TcpListener, TcpStream};
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    /// The name is not part of the trust decision — the pinned key is — so the
    /// tests use one that can never resolve, which would fail loudly if some
    /// future change started relying on it.
    const UNUSED_NAME: &str = "lan.wisp.invalid";

    /// Runs a real handshake over loopback and reports what each side saw.
    ///
    /// Returns `(identity the server read, identity the client read,
    /// negotiated version)`.
    async fn handshake(
        server_secret: SecretKey,
        client_secret: SecretKey,
        client_expects: PublicKey,
    ) -> std::result::Result<(PublicKey, PublicKey, Option<rustls::ProtocolVersion>), String> {
        let acceptor = TlsAcceptor::from(server_config(&server_secret).map_err(err)?);
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(err)?;
        let addr = listener.local_addr().map_err(err)?;

        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.map_err(err)?;
            let tls = acceptor.accept(tcp).await.map_err(err)?;
            let (_, connection) = tls.get_ref();
            peer_identity(connection.peer_certificates()).map_err(err)
        });

        let connector =
            TlsConnector::from(client_config(&client_secret, client_expects).map_err(err)?);
        let tcp = TcpStream::connect(addr).await.map_err(err)?;
        let name = ServerName::try_from(UNUSED_NAME).map_err(err)?;
        let tls = connector.connect(name, tcp).await.map_err(err)?;
        let (_, connection) = tls.get_ref();
        let seen_by_client = peer_identity(connection.peer_certificates()).map_err(err)?;
        let version = connection.protocol_version();

        let seen_by_server = server.await.map_err(err)??;
        Ok((seen_by_server, seen_by_client, version))
    }

    fn err(e: impl std::fmt::Display) -> String {
        e.to_string()
    }

    #[tokio::test]
    async fn both_ends_learn_the_other_identity() {
        let server = SecretKey::generate();
        let client = SecretKey::generate();
        // The version is asserted by `negotiates_tls13_only`.
        let (seen_by_server, seen_by_client, _) =
            handshake(server.clone(), client.clone(), server.public())
                .await
                .expect("handshake should succeed");

        assert_eq!(seen_by_server, client.public());
        assert_eq!(seen_by_client, server.public());
        // Mutual auth is the point: a server that learned nothing about the
        // client could not decide whether to accept the transfer.
        assert_ne!(seen_by_server, seen_by_client);
    }

    #[tokio::test]
    async fn negotiates_tls13_only() {
        let server = SecretKey::generate();
        let client = SecretKey::generate();
        let (_, _, version) = handshake(server.clone(), client, server.public())
            .await
            .expect("handshake should succeed");
        assert_eq!(version, Some(rustls::ProtocolVersion::TLSv1_3));
    }

    #[tokio::test]
    async fn a_client_expecting_another_peer_is_refused() {
        let server = SecretKey::generate();
        let client = SecretKey::generate();
        // The attacker holds a valid identity of its own; what it does not hold
        // is the one the client was told to expect.
        let impostor = SecretKey::generate();

        let result = handshake(server, client, impostor.public()).await;
        assert!(
            result.is_err(),
            "pinning must reject a peer that is not the expected one, got {result:?}"
        );
    }

    #[test]
    fn peer_identity_rejects_malformed_keys() {
        assert!(peer_identity(None).is_err(), "no certificate at all");
        assert!(peer_identity(Some(&[])).is_err(), "empty certificate list");

        let key = SecretKey::generate();
        let spki = rustls::sign::public_key_to_spki(&alg_id::ED25519, key.public().as_bytes());
        let good = CertificateDer::from(spki.as_ref().to_vec());
        assert_eq!(
            peer_identity(Some(std::slice::from_ref(&good))).expect("a raw ed25519 key"),
            key.public()
        );

        // A chain is not a raw public key, whatever the leaf is.
        assert!(
            peer_identity(Some(&[good.clone(), good.clone()])).is_err(),
            "chain"
        );

        // Right length, wrong algorithm prefix: rebuilding the SPKI from the
        // tail is what catches this, and it is the check worth having.
        let mut wrong_alg = spki.as_ref().to_vec();
        wrong_alg[3] ^= 0xff;
        assert!(
            peer_identity(Some(&[CertificateDer::from(wrong_alg)])).is_err(),
            "non-ed25519 algorithm identifier"
        );

        assert!(
            peer_identity(Some(&[CertificateDer::from(vec![0u8; 8])])).is_err(),
            "too short to hold a key"
        );
    }
}
