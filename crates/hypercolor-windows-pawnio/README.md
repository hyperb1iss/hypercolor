# hypercolor-windows-pawnio

*SMBus access on Windows via the PawnIO kernel driver.*

Linux exposes SMBus controllers through `/dev/i2c-*` (the `i2cdev` crate).
Windows has no equivalent user-space API — accessing SMBus registers requires
a kernel driver. This crate wraps
[namazso/PawnIO](https://github.com/namazso/PawnIO), a third-party kernel
driver and user-space runtime that exposes SMBus through loadable `.bin`
module blobs (one per controller family: `SmbusI801`, `SmbusPIIX4`,
`SmbusNCT6793`). The crate handles runtime DLL loading (`PawnIOLib.dll`),
module blob loading and IOCTL dispatch, bus enumeration, and a global
cross-process SMBus mutex (`Global\Access_SMBUS.HTP.Method`).

Because direct PawnIO access requires elevated privileges or specific device
ACLs, the crate also ships a Windows Service binary
(`hypercolor-smbus-service`) that acts as a local broker. Non-elevated
Hypercolor processes connect via a named pipe rather than calling PawnIO
directly. The `HYPERCOLOR_PAWNIO_DIRECT=1` environment variable forces direct
mode when needed.

**Platform scope.** This crate compiles on all platforms. On non-Windows
targets, stub types and functions are defined inline in `lib.rs`, each
returning `PawnIoError::UnsupportedPlatform`. This keeps `hypercolor-hal`'s
build clean on Linux without requiring feature flags.

**Safety.** `unsafe_code = "allow"` is set for this crate due to raw DLL and
IOCTL FFI. `undocumented_unsafe_blocks = "deny"` and `unwrap_used = "deny"`
are both set as compensating controls.

## Workspace position

**Depends on:** `libloading`, `thiserror`, `tracing`; Windows-only: `anyhow`,
`serde`, `serde_json`, `tokio`, `tracing-subscriber`, `windows-registry`,
`windows-service`, `windows-sys`.

**Depended on by:** `hypercolor-hal` (Windows only, via
`[target.'cfg(target_os = "windows")'.dependencies]`).

**Ships:** `hypercolor-smbus-service` — the Windows Service broker binary.

## Key types and entry points

**Bus enumeration and access**

- `enumerate_smbus_buses() -> PawnIoResult<Vec<WindowsSmBusBusInfo>>` —
  discovers all available PawnIO SMBus buses. Tries the broker first, falls
  back to direct PawnIO.
- `open_smbus_bus(path: &str) -> PawnIoResult<WindowsSmBusBus>` — opens one
  bus by path (`pawnio:{module}` or `pawnio:{module}:{port}`).
- `WindowsSmBusBus` — open bus handle. Methods: `info()`, `smbus_xfer()`,
  `smbus_xfer_batch()`, `probe_quick_write()`, `probe_presence()`. Implements
  `Send + Sync`.
- `WindowsSmBusBusInfo` — bus metadata: path string, module name, optional
  PIIX4 port, PCI identity fields, resolved module path.

**Transfer types**

- `SmBusTransaction` — `Quick`, `Byte`, `ByteData`, `WordData`, `BlockData`.
- `SmBusBatchOperation` — `Transfer { direction, command, transaction }` or
  `Delay { duration }` for batched bus operations.
- `SmBusBlockData` — 32-byte-max block payload wrapper.
- `SmBusDirection` — `Read` / `Write`.

**Service entry point**

- `run_smbus_service() -> anyhow::Result<()>` — Windows Service entry point
  called by the `hypercolor-smbus-service` binary. This is the only broker
  API consumers need; the broker's IPC protocol is internal.

**Errors**

- `PawnIoError` — covers: installation not found, module not found, load
  failures, HRESULT-mapped call failures, bus I/O errors, invalid input,
  broker unavailable/failure, unsupported platform.

## Feature flags

None.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source
RGB lighting orchestration for Linux. Licensed under Apache-2.0.
