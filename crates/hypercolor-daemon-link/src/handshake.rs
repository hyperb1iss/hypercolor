use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::identity::{IdentityKeypair, IdentitySignature};

pub const DAEMON_CONNECT_PATH: &str = "/v1/daemon/connect";
pub const UPGRADE_METHOD: &str = "GET";
pub const HEADER_AUTHORIZATION: &str = "Authorization";
pub const HEADER_WEBSOCKET_PROTOCOL: &str = "Sec-WebSocket-Protocol";
pub const HEADER_DAEMON_ID: &str = "X-Hypercolor-Daemon-Id";
pub const HEADER_DAEMON_VERSION: &str = "X-Hypercolor-Daemon-Version";
pub const HEADER_DAEMON_TS: &str = "X-Hypercolor-Daemon-Ts";
pub const HEADER_DAEMON_NONCE: &str = "X-Hypercolor-Daemon-Nonce";
pub const HEADER_DAEMON_SIG: &str = "X-Hypercolor-Daemon-Sig";

#[derive(Clone, PartialEq, Eq)]
pub struct UpgradeSignatureInput<'a> {
    pub method: &'a str,
    pub host: &'a str,
    pub path: &'a str,
    pub websocket_protocol: &'a str,
    pub daemon_id: Uuid,
    pub daemon_version: &'a str,
    pub timestamp: &'a str,
    pub nonce: &'a str,
    pub authorization_jwt: &'a str,
}

impl std::fmt::Debug for UpgradeSignatureInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpgradeSignatureInput")
            .field("method", &self.method)
            .field("host", &self.host)
            .field("path", &self.path)
            .field("websocket_protocol", &self.websocket_protocol)
            .field("daemon_id", &self.daemon_id)
            .field("daemon_version", &self.daemon_version)
            .field("timestamp", &self.timestamp)
            .field("nonce", &self.nonce)
            .field("authorization_jwt", &"<redacted>")
            .finish()
    }
}

impl UpgradeSignatureInput<'_> {
    #[must_use]
    pub fn canonicalize(&self) -> CanonicalUpgrade {
        let authorization_hash = Sha256::digest(self.authorization_jwt.as_bytes());
        let authorization_hash = STANDARD.encode(authorization_hash);
        let bytes = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            self.method,
            self.host,
            self.path,
            self.websocket_protocol,
            self.daemon_id,
            self.daemon_version,
            self.timestamp,
            self.nonce,
            authorization_hash
        );
        let sha256 = Sha256::digest(bytes.as_bytes()).into();

        CanonicalUpgrade { bytes, sha256 }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct UpgradeHeaderInput<'a> {
    pub host: &'a str,
    pub daemon_id: Uuid,
    pub daemon_version: &'a str,
    pub timestamp: &'a str,
    pub nonce: &'a UpgradeNonce,
    pub authorization_jwt: &'a str,
}

impl std::fmt::Debug for UpgradeHeaderInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpgradeHeaderInput")
            .field("host", &self.host)
            .field("daemon_id", &self.daemon_id)
            .field("daemon_version", &self.daemon_version)
            .field("timestamp", &self.timestamp)
            .field("nonce", &self.nonce)
            .field("authorization_jwt", &"<redacted>")
            .finish()
    }
}

impl UpgradeHeaderInput<'_> {
    #[must_use]
    pub fn signed_headers(&self, keypair: &IdentityKeypair) -> SignedUpgradeHeaders {
        let canonical = UpgradeSignatureInput {
            method: UPGRADE_METHOD,
            host: self.host,
            path: DAEMON_CONNECT_PATH,
            websocket_protocol: crate::WEBSOCKET_PROTOCOL,
            daemon_id: self.daemon_id,
            daemon_version: self.daemon_version,
            timestamp: self.timestamp,
            nonce: self.nonce.as_str(),
            authorization_jwt: self.authorization_jwt,
        }
        .canonicalize();
        let signature = keypair.sign(canonical.as_bytes());

        SignedUpgradeHeaders {
            authorization: format!("Bearer {}", self.authorization_jwt),
            websocket_protocol: crate::WEBSOCKET_PROTOCOL.to_owned(),
            daemon_id: self.daemon_id.hyphenated().to_string(),
            daemon_version: self.daemon_version.to_owned(),
            timestamp: self.timestamp.to_owned(),
            nonce: self.nonce.as_str().to_owned(),
            signature,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SignedUpgradeHeaders {
    pub authorization: String,
    pub websocket_protocol: String,
    pub daemon_id: String,
    pub daemon_version: String,
    pub timestamp: String,
    pub nonce: String,
    pub signature: IdentitySignature,
}

impl std::fmt::Debug for SignedUpgradeHeaders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedUpgradeHeaders")
            .field("authorization", &"<redacted>")
            .field("websocket_protocol", &self.websocket_protocol)
            .field("daemon_id", &self.daemon_id)
            .field("daemon_version", &self.daemon_version)
            .field("timestamp", &self.timestamp)
            .field("nonce", &self.nonce)
            .field("signature", &self.signature)
            .finish()
    }
}

impl SignedUpgradeHeaders {
    #[must_use]
    pub fn pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            (HEADER_AUTHORIZATION, self.authorization.clone()),
            (HEADER_WEBSOCKET_PROTOCOL, self.websocket_protocol.clone()),
            (HEADER_DAEMON_ID, self.daemon_id.clone()),
            (HEADER_DAEMON_VERSION, self.daemon_version.clone()),
            (HEADER_DAEMON_TS, self.timestamp.clone()),
            (HEADER_DAEMON_NONCE, self.nonce.clone()),
            (HEADER_DAEMON_SIG, self.signature.as_str().to_owned()),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradeNonce(String);

impl UpgradeNonce {
    pub fn new(encoded: impl Into<String>) -> Result<Self, UpgradeNonceError> {
        validate_base64_len(&encoded.into(), 16).map(Self)
    }

    #[must_use]
    pub fn generate() -> Self {
        let mut bytes = [0_u8; 16];
        OsRng.fill_bytes(&mut bytes);
        Self::from_bytes(bytes)
    }

    #[must_use]
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(STANDARD.encode(bytes))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for UpgradeNonce {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalUpgrade {
    bytes: String,
    sha256: [u8; 32],
}

impl CanonicalUpgrade {
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.as_bytes()
    }

    #[must_use]
    pub const fn sha256(&self) -> [u8; 32] {
        self.sha256
    }
}

fn validate_base64_len(encoded: &str, expected: usize) -> Result<String, UpgradeNonceError> {
    let decoded = STANDARD
        .decode(encoded)
        .map_err(UpgradeNonceError::Decode)?;
    if decoded.len() != expected {
        return Err(UpgradeNonceError::InvalidLength {
            expected,
            actual: decoded.len(),
        });
    }
    Ok(encoded.to_owned())
}

#[derive(Debug, thiserror::Error)]
pub enum UpgradeNonceError {
    #[error("invalid base64 upgrade nonce: {0}")]
    Decode(#[source] base64::DecodeError),
    #[error("invalid upgrade nonce length: expected {expected} bytes, got {actual}")]
    InvalidLength { expected: usize, actual: usize },
}
