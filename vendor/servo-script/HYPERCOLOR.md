# Hypercolor patches to servo-script

This directory vendors `servo-script` 0.6.0, Servo's `components/script`
crate, from crates.io. The files are the published package exactly as
`servo-script-0.6.0.crate` unpacks (checksum
`c08de23e00326c3f698ebe2a39bc4d51fbf7507af128c8cbb295b1ae6ae2f13c`, upstream
commit `c78d2c206f80a1c8b67eefa97f773bba513205d3` per
`.cargo_vcs_info.json`), plus this file. The root `Cargo.toml` swaps it in
through `[patch.crates-io]`, so every other Servo crate still comes from
crates.io. The crate is excluded from the workspace and keeps its own
formatting, lints, and MPL-2.0 license headers.

The commit that added this directory carries no source edits. Each patch
below lands in its own later commit, so `git log -p -- vendor/servo-script`
shows every change against the published crate.

## Patches

None yet.

## Removal rule

Drop this directory and the `[patch.crates-io]` entry once a Servo release
carries equivalent fixes. Until then, bumping Servo means re-vendoring
`servo-script` at the new version and reapplying each patch above (or
confirming upstream made it unnecessary) in the same change. A patch entry
for 0.6.0 does nothing for any other `servo-script` version: Cargo only
warns that the patch went unused and builds the unpatched crate from
crates.io. Dependabot ignores `servo-script` and minor `servo` bumps for that
reason.

`[patch]` only applies to builds of this workspace. A `hypercolor-core`
published to crates.io would build against the unpatched registry crate and
lose every patch listed above until a Servo release carries them.
