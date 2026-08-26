# hypercolor-platform-fs

*The audited filesystem boundary — durable replacement, secret files, and
per-user directories.*

Every filesystem operation whose correctness depends on the operating system
lives here rather than being reimplemented per crate. That covers atomic
replacement with a real durability barrier, creating files that must never be
world-readable, opening a path without following a symlink, and resolving the
per-user config, data, and cache directories. Callers above this crate never
branch on the operating system for any of it.

## Safety

`unsafe_code = "allow"` is set because the Windows path needs Win32 FFI. It is
narrowly scoped: `lib.rs` carries
`#![cfg_attr(not(target_os = "windows"), forbid(unsafe_code))]`, so on Unix the
crate is a hard `forbid`. `#![deny(unsafe_op_in_unsafe_fn)]`,
`clippy::undocumented_unsafe_blocks = "deny"`, and `clippy::unwrap_used = "deny"`
are the compensating controls.

## Workspace position

**Depends on:** `dirs`; `rustix` and `sha2` on Unix; `windows-sys` on Windows.
No Hypercolor crates.

**Depended on by:** `hypercolor-core`, `hypercolor-daemon`, `hypercolor-cli`,
`hypercolor-driver-api`, `hypercolor-driver-support`, `hypercolor-persistence`,
and `hypercolor-macos-owner`.

## Key entry points

**Free functions**

- `durable_replace(source, destination)` — atomically replaces the destination
  and makes the replacement durable. Windows requests write-through durability
  from the OS; Unix syncs the destination's parent directory before returning.
- `replace_file(source, destination)` — atomic replacement without the final
  durability barrier.
- `write_secret(path, contents)` — creates a file that must not already exist,
  writes it, and syncs. Unix files are created mode `0600`. A failed write or
  sync removes the new file on a best-effort basis.
- `open_no_follow(path)` — opens a path while refusing to traverse a symlink.

**Per-user directories** (`user_dirs`)

- `config_base_dir()`, `data_base_dir()`, `app_cache_dir(app)`. On Linux the
  XDG base-directory variables are honoured verbatim, with the conventional
  dotfolder under `$HOME` as the fallback; other platforms resolve through the
  OS's own conventions. Callers append their own application segment.

**Unix directory authorities**

- `DirectoryAuthority`, `PublicDirectoryAuthority`, `ReadOnlyDirectoryAuthority`,
  `ExclusiveDirectory`, and `PrivateStagingDirectory` — handle-scoped directory
  access with bounded child counts (`MAX_PUBLIC_DIRECTORY_CHILD_COUNT`,
  `MAX_PUBLIC_DIRECTORY_CHILD_NAMES_BYTES`).
- `ExactEntry`, `ExactDirectoryEntry`, `EntryReplacement`, `OpenedRegularFile`,
  `DirectoryEntryKind`, `DirectoryEntryMetadata`, `MAX_EXACT_ENTRY_BYTES`.

**Windows**

- `DestinationIdentity` — pins a replacement destination by parent volume
  serial, parent file id, file name, and case sensitivity, so a replacement
  cannot land on a different file than the one that was resolved.

## Feature flags

None. Platform behavior is selected by `[target]` dependencies and `cfg`, not by
features.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source
RGB lighting orchestration for Linux, Windows, and macOS. Licensed under
Apache-2.0.
