//! Multiplexed WebSocket tunnel protocol and ed25519 identity layer for Hypercolor Cloud.
//!
//! Defines daemon identity keypairs, the signed HTTP upgrade handshake, binary
//! frame wire format, channel naming, and connection admission. Sits between
//! `hypercolor-cloud-api` (REST types) and `hypercolor-cloud-client` (high-level
//! daemon client). Protocol: `hypercolor-daemon.v1`, version `1`.

#![forbid(unsafe_code)]

pub mod admission;
pub mod channel;
pub mod frame;
pub mod handshake;
pub mod identity;

pub use admission::{AdmissionError, AdmissionSet};
pub use channel::{ChannelName, ChannelParseError};
pub use frame::{
    DaemonCapabilities, DeniedChannel, Frame, FrameKind, HelloFrame, ServerCapabilities,
    WelcomeFrame,
};
pub use handshake::{
    CanonicalUpgrade, DAEMON_CONNECT_PATH, HEADER_AUTHORIZATION, HEADER_DAEMON_ID,
    HEADER_DAEMON_NONCE, HEADER_DAEMON_SIG, HEADER_DAEMON_TS, HEADER_DAEMON_VERSION,
    HEADER_WEBSOCKET_PROTOCOL, SignedUpgradeHeaders, UPGRADE_METHOD, UpgradeHeaderInput,
    UpgradeNonce, UpgradeNonceError, UpgradeSignatureInput,
};
pub use identity::{
    IdentityEncodingError, IdentityKeypair, IdentityNonce, IdentityPrivateKey, IdentityPublicKey,
    IdentitySignature, IdentityVerificationError, registration_proof_message,
    verify_identity_signature,
};

pub const PROTOCOL_VERSION: u16 = 1;
pub const WEBSOCKET_PROTOCOL: &str = "hypercolor-daemon.v1";
