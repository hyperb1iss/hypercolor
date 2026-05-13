<!-- Keep this tight. One or two sentences per section is usually enough. -->

## What this changes

<!-- What behavior or surface does this PR touch? -->

## Why

<!-- Motivation. Issue number, spec reference, or a short reason. -->

## Verification

<!-- Check every gate that matches the files this PR touched. -->

- [ ] Added or updated tests
- [ ] Added or updated docs (README, AGENTS.md, relevant spec, or guide)
- [ ] `just verify` passes locally (Rust fmt + lint + test)
- [ ] `just ui-test` and `just ui-build` pass (required for `crates/hypercolor-ui/`)
- [ ] `just sdk-lint`, `just sdk-check`, and `just sdk-build` pass (required for `sdk/`)
- [ ] `just python-verify` passes (required for `python/`)
- [ ] `just compat-check` passes (required for `data/drivers/vendors/*.toml`)
- [ ] `just docs-build` passes (required for docs or README changes)
- [ ] Packaging scripts were syntax-checked (required for `scripts/` or `packaging/`)
- [ ] Tested on real hardware, simulator, or e2e harness (describe below)

## Notes for reviewers

<!-- Anything tricky, unusual, or worth flagging. Delete this section if nothing. -->
