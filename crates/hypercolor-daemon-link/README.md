# hypercolor-daemon-link

*Multiplexed WebSocket tunnel protocol and ed25519 identity layer for
Hypercolor Cloud.*

When a Hypercolor daemon connects to the cloud relay, it does not use a plain
WebSocket. It authenticates itself with a signed HTTP upgrade handshake, then
communicates over a multiplexed binary frame protocol on top of that WebSocket.
This crate owns every byte of that protocol: the identity keypairs, the signed
headers, the channel naming scheme, and the frame wire format.

## Role and Position

`hypercolor-daemon-link` sits in the middle of the three-crate cloud stack:

```
hypercolor-cloud-api   (REST contract types — no I/O)
       ↓
hypercolor-daemon-link ← you are here (WebSocket framing, identity, signing)
       ↓
hypercolor-cloud-client (daemon-side OAuth, keyring, sync — used by the daemon)
```

Depends on `hypercolor-cloud-api` for shared type primitives. Consumed by
`hypercolor-cloud-client`, which uses this crate's `SignedUpgradeHeaders` and
`Frame` types to open and operate the tunnel.

Additional dependencies: `ed25519-dalek`, `sha2`, `rand_core`, `base64`,
`serde`, `serde_json`, `ulid`, `uuid`.

## Public Surface

All types are re-exported at the crate root:

- **Identity** — `IdentityKeypair`, `IdentityPrivateKey`, `IdentityPublicKey`,
  `IdentitySignature`, `IdentityNonce`, `IdentityEncodingError`,
  `IdentityVerificationError`, `registration_proof_message`,
  `verify_identity_signature`.
- **Handshake** — `SignedUpgradeHeaders`, `UpgradeSignatureInput`,
  `CanonicalUpgrade`, `UpgradeHeaderInput`, `UpgradeNonce`, `UpgradeNonceError`,
  plus header name constants (`HEADER_DAEMON_ID`, `HEADER_DAEMON_SIG`, etc.) and
  `DAEMON_CONNECT_PATH`, `UPGRADE_METHOD`.
- **Frame** — `Frame`, `FrameKind`, `HelloFrame`, `WelcomeFrame`,
  `DaemonCapabilities`, `ServerCapabilities`, `DeniedChannel`.
- **Channel** — `ChannelName`, `ChannelParseError`.
- **Admission** — `AdmissionSet`, `AdmissionError`.
- **Protocol constants** — `PROTOCOL_VERSION: u16 = 1`,
  `WEBSOCKET_PROTOCOL: &str = "hypercolor-daemon.v1"`.

## Cargo Features

None. This crate is always compiled in full.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB
lighting orchestration for Linux. Apache-2.0.
