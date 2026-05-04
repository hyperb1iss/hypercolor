use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
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
