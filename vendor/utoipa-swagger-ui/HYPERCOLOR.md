# Hypercolor patches to utoipa-swagger-ui

This directory vendors `utoipa-swagger-ui` 10.0.1 from crates.io. The files
are the published package exactly as `utoipa-swagger-ui-10.0.1.crate`
unpacks (checksum
`3f5ec9d7816e4ce8ccb7e0bd7276e090cad0fcb1def4f3540e5b0f7485d7583e`, upstream
commit `4225d2ebefa86696affc69fdd5b38a8c38cb3eca` per
`.cargo_vcs_info.json`), plus this file. The root `Cargo.toml` swaps it in
through `[patch.crates-io]`, so `utoipa`, `utoipa-axum`, and
`utoipa-swagger-ui-vendored` still come from crates.io. The crate is
excluded from the workspace and keeps upstream's formatting and license
files.

The commit that added this directory carries no source edits. Each patch
below lands in its own later commit, so `git log -p --
vendor/utoipa-swagger-ui` shows every change against the published crate.

The daemon uses the crate for one route: `SwaggerUi::new("/api/v1/docs")`
in `crates/hypercolor-daemon/src/api/openapi.rs`, built with the `axum` and
`vendored` features. Only the build script touches the patched code; it
unpacks the Swagger UI archive shipped by `utoipa-swagger-ui-vendored` into
`OUT_DIR` at compile time.

## Removal rule

Drop this directory and its `[patch.crates-io]` entry once a
`utoipa-swagger-ui` release carries every patch below. A patch entry for
10.0.1 does nothing for any other version: Cargo only warns that the patch
went unused and builds the unpatched crate from crates.io.
`scripts/check-vendored-patches.sh` turns that into a CI failure, so any
update that moves `utoipa-swagger-ui` past 10.0.1 (Dependabot's monthly
`rust` group or a hand bump) fails until it also deletes the vendor, when
the new release carries the patches, or re-vendors the new version with
them reapplied.

`[patch]` only applies to builds of this workspace. A `hypercolor-daemon`
published to crates.io would build against the unpatched registry crate.
