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
