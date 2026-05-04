use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdentityPublicKey(String);

impl IdentityPublicKey {
    pub fn new(encoded: impl Into<String>) -> Result<Self, IdentityEncodingError> {
        validate_base64_len(&encoded.into(), 32).map(Self)
    }

    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(STANDARD.encode(bytes))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn decode(&self) -> Result<[u8; 32], IdentityEncodingError> {
        decode_fixed(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdentityPrivateKey(String);

impl IdentityPrivateKey {
    pub fn new(encoded: impl Into<String>) -> Result<Self, IdentityEncodingError> {
        validate_base64_len(&encoded.into(), 32).map(Self)
    }

    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(STANDARD.encode(bytes))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn decode(&self) -> Result<[u8; 32], IdentityEncodingError> {
        decode_fixed(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdentityNonce(String);

impl IdentityNonce {
    pub fn new(encoded: impl Into<String>) -> Result<Self, IdentityEncodingError> {
        validate_base64_len(&encoded.into(), 32).map(Self)
    }

    #[must_use]
    pub fn generate() -> Self {
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self::from_bytes(bytes)
    }

    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(STANDARD.encode(bytes))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn decode(&self) -> Result<[u8; 32], IdentityEncodingError> {
        decode_fixed(&self.0)
    }
}

impl AsRef<str> for IdentityNonce {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdentitySignature(String);

impl IdentitySignature {
    pub fn new(encoded: impl Into<String>) -> Result<Self, IdentityEncodingError> {
        validate_base64_len(&encoded.into(), 64).map(Self)
    }

    #[must_use]
    pub fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(STANDARD.encode(bytes))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn decode(&self) -> Result<[u8; 64], IdentityEncodingError> {
        decode_fixed(&self.0)
    }
}

#[derive(Debug, Clone)]
pub struct IdentityKeypair {
    signing_key: SigningKey,
}

impl IdentityKeypair {
    #[must_use]
    pub fn generate() -> Self {
        Self {
            signing_key: SigningKey::generate(&mut OsRng),
        }
    }

    pub fn from_private_key(
        private_key: &IdentityPrivateKey,
    ) -> Result<Self, IdentityEncodingError> {
        Ok(Self {
            signing_key: SigningKey::from_bytes(&private_key.decode()?),
        })
    }

    #[must_use]
    pub fn private_key(&self) -> IdentityPrivateKey {
        IdentityPrivateKey::from_bytes(self.signing_key.to_bytes())
    }

    #[must_use]
    pub fn public_key(&self) -> IdentityPublicKey {
        IdentityPublicKey::from_bytes(self.signing_key.verifying_key().to_bytes())
    }

    #[must_use]
    pub fn sign(&self, message: &[u8]) -> IdentitySignature {
        IdentitySignature::from_bytes(self.signing_key.sign(message).to_bytes())
    }
}

#[must_use]
pub fn registration_proof_message(
    daemon_id: Uuid,
    identity_pubkey: &IdentityPublicKey,
    nonce: impl AsRef<str>,
) -> Vec<u8> {
    [
        daemon_id.hyphenated().to_string(),
        identity_pubkey.as_str().to_owned(),
        nonce.as_ref().to_owned(),
    ]
    .join("\n")
    .into_bytes()
}

pub fn verify_identity_signature(
    public_key: &IdentityPublicKey,
    message: &[u8],
    signature: &IdentitySignature,
) -> Result<(), IdentityVerificationError> {
    let verifying_key = VerifyingKey::from_bytes(&public_key.decode()?)?;
    let signature = Signature::from_bytes(&signature.decode()?);
    verifying_key.verify(message, &signature)?;
    Ok(())
}

fn validate_base64_len(encoded: &str, expected: usize) -> Result<String, IdentityEncodingError> {
    let decoded = STANDARD
        .decode(encoded)
        .map_err(IdentityEncodingError::Decode)?;
    if decoded.len() != expected {
        return Err(IdentityEncodingError::InvalidLength {
            expected,
            actual: decoded.len(),
        });
    }
    Ok(encoded.to_owned())
}

fn decode_fixed<const N: usize>(encoded: &str) -> Result<[u8; N], IdentityEncodingError> {
    let decoded = STANDARD
        .decode(encoded)
        .map_err(IdentityEncodingError::Decode)?;
    decoded
        .try_into()
        .map_err(|decoded: Vec<u8>| IdentityEncodingError::InvalidLength {
            expected: N,
            actual: decoded.len(),
        })
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityEncodingError {
    #[error("invalid base64 identity material: {0}")]
    Decode(#[source] base64::DecodeError),
    #[error("invalid identity material length: expected {expected} bytes, got {actual}")]
    InvalidLength { expected: usize, actual: usize },
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityVerificationError {
    #[error(transparent)]
    Encoding(#[from] IdentityEncodingError),
    #[error("identity signature verification failed: {0}")]
    Signature(#[from] ed25519_dalek::SignatureError),
}
