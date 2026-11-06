# hypercolor-driver-support

*Native host services shared by Hypercolor driver implementations.*

This crate implements concrete utilities on top of the stable
`hypercolor-driver-api` contracts. Drivers use it for native credentials,
discovery, control documents, and pairing lifecycle policy. Capability traits
and wire values remain in `hypercolor-driver-api`.

## Position in the Workspace

- Depends on `hypercolor-driver-api`, `hypercolor-types`,
  `hypercolor-persistence`, and `hypercolor-platform-fs`; never
  `hypercolor-core` or the daemon.
- Consumed by network drivers, `hypercolor-driver-builtin`, and the daemon.
- Owns native dependencies such as `aes-gcm`, `mdns-sd`, `reqwest`, and
  filesystem I/O. Drivers reach HTTP through this crate rather than depending
  on `reqwest`, which keeps its unbounded readers out of driver code.

## Public Surface

- `CredentialStore` implements `DriverCredentialStore` with encrypted,
  driver-scoped JSON values and private file permissions.
- `MdnsBrowser` resolves deterministic IPv4 service endpoints, keeping a
  bounded set of hosts beyond the ones the caller already knows.
- `MdnsService` carries one resolved endpoint and its TXT properties.
- `discovery` bounds a scan: `CandidateAdmission` caps what unsolicited replies
  can make it retain and gives known targets priority, `probe_candidates`
  bounds concurrency under a deadline, and `ScanOutcome` carries what they
  found beside the coverage gaps those bounds cost.
- `control_apply` validates and persists driver-owned control changes.
- `control_surface` builds canonical control documents and revisions.
- `framing` reads bounded lines on absolute deadlines and writes whole frames
  through a writer that gives up its stream when one fails.
- `http` runs every exchange on a `ReadBudget`, an absolute deadline plus a
  byte cap enforced as body chunks arrive.
- `network` validates endpoints and extracts typed discovery metadata.
- `pairing` coordinates post-pair activation and post-unpair disconnects.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor), open-source RGB
lighting orchestration for Linux, Windows, and macOS. Licensed under
Apache-2.0.
