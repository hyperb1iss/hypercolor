# hypercolor-driver-support

*Native host services shared by Hypercolor driver implementations.*

This crate implements concrete utilities on top of the stable
`hypercolor-driver-api` contracts. Drivers use it for native credentials,
discovery, control documents, and pairing lifecycle policy. Capability traits
and wire values remain in `hypercolor-driver-api`.

## Position in the Workspace

- Depends on `hypercolor-driver-api`, never `hypercolor-core` or the daemon.
- Consumed by network drivers, `hypercolor-driver-builtin`, and the daemon.
- Owns native dependencies such as `aes-gcm`, `mdns-sd`, and filesystem I/O.

## Public Surface

- `CredentialStore` implements `DriverCredentialStore` with encrypted,
  driver-scoped JSON values and private file permissions.
- `MdnsBrowser` resolves deterministic IPv4 service endpoints.
- `MdnsService` carries one resolved endpoint and its TXT properties.
- `control_apply` validates and persists driver-owned control changes.
- `control_surface` builds canonical control documents and revisions.
- `network` validates endpoints and extracts typed discovery metadata.
- `pairing` coordinates post-pair activation and post-unpair disconnects.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor), open-source RGB
lighting orchestration for Linux. Licensed under Apache-2.0.
