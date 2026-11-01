# Hypercolor patches to libsqlite3-sys

This directory vendors `libsqlite3-sys` 0.36.0 from crates.io. The files are
the published package exactly as `libsqlite3-sys-0.36.0.crate` unpacks
(checksum
`95b4103cffefa72eb8428cb6b47d6627161e51c2739fc5e3b734584157bc642a`, upstream
commit `35b3be2436a63d21701d1d110661e6392831fea0` per
`.cargo_vcs_info.json`), plus this file. The root `Cargo.toml` swaps it in
through `[patch.crates-io]`, so `rusqlite` still comes from crates.io. The
crate is excluded from the workspace and keeps upstream's formatting and
license file. The packaged `Cargo.lock` is committed even though the crate's
own `.gitignore` names it, so the directory matches the package byte for
byte.

The commit that added this directory carries no source edits. Each patch
below lands in its own later commit, so `git log -p --
vendor/libsqlite3-sys` shows every change against the published crate.

The engine never names the crate. `servo-net` 0.6.0 depends on `rusqlite`
^0.38 with the `bundled` feature, and `rusqlite` 0.38 depends on this crate,
so the daemon compiles the SQLite amalgamation under `sqlite3/` into the
process. Products built on the engine that use `rusqlite` themselves link the
same copy, because only one crate in a build may link `sqlite3`. That also
means none of them can move to a newer `rusqlite` until `servo-net` does.

## Patches

### 1. Bundle SQLite 3.51.3

Files: `sqlite3/sqlite3.c`, `sqlite3/sqlite3.h`,
`sqlite3/bindgen_bundled_version.rs`, and
`sqlite3/bindgen_bundled_version_ext.rs`.

The four files are copied unchanged from `libsqlite3-sys` 0.37.0, which
bundles SQLite 3.51.3. The SHA3-256 of `sqlite3.c` is
`32d5424f97e0a7fc5ed2f6335afbb58be4e0298bd7117a34e39d345ff13d859e`, the
value SQLite publishes for the 3.51.3 amalgamation. Between 3.51.1 and
3.51.3, `sqlite3.h` changes only its version macros and `sqlite3ext.h` does
not change, so the 0.36.0 bindings stay correct. The two bindings files
differ from 0.36.0 only in the five version constants, which now report
3.51.3.

SQLite's patch releases between the two versions fix:

- **Deadlock (3.51.2).** In 3.51.0 and 3.51.1, the check for broken POSIX
  locks that runs while a connection closes a WAL database takes the VFS-wide
  mutex while holding the file's lock mutex. Opening a file takes the two in
  the opposite order, in `findReusableFd`. A connection closing one database
  while another thread opens the same file deadlocks both, and every later
  SQLite call in the process then blocks on the VFS-wide mutex. Upstream
  check-in `f65aa46796068800905f6ff25efcb6338f864ce5bf8dcf6c7b9f5613b3973ab2`
  fixes the lock order.
- **Wrong query results (3.51.2).** Several bugs in the EXISTS-to-JOIN
  optimization added in 3.51.0.
- **WAL corruption (3.51.3).** The WAL-reset database corruption bug.

`sqlcipher/` keeps the published SQLCipher amalgamation. Only the
`bundled-sqlcipher` features compile it, and nothing in the engine enables
them.

## When to drop the vendor

Drop this directory and its `[patch.crates-io]` entry once `servo-net` moves
to a `rusqlite` whose `libsqlite3-sys` bundles SQLite 3.51.3 or newer
(`rusqlite` 0.39 and later do). `scripts/check-vendored-patches.sh` fails CI
as soon as a bump moves the crate off 0.36.0.
