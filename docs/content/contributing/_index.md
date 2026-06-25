+++
title = "Contributing"
description = "Dev setup, the quality gates, commit scopes, the effect-contribution path, and the spec-writing walkthrough."
weight = 90
sort_by = "weight"
template = "section.html"
+++

Hypercolor is open source under Apache-2.0, and contributions are welcome across the whole stack: bug fixes, new device drivers, effects, documentation, and feature work. This section is your onboarding guide.

## In this section

- [Debugging](@/contributing/debugging.md) — diagnosing render pipeline, USB, and daemon issues
- [Adding an effect](@/contributing/adding-an-effect.md) — TypeScript SDK and native Rust paths
- [Adding a HAL driver](@/contributing/adding-a-driver.md) — USB/HID/SMBus protocol implementation
- [Adding a network driver](@/contributing/adding-a-network-driver.md) — Hue, WLED, Govee, Nanoleaf and the driver-api boundary

---

## Development setup

```bash
# Clone the repo
git clone https://github.com/hyperb1iss/hypercolor.git
cd hypercolor

# Install Rust toolchain, Bun (for SDK + scripts), and frontend deps
just setup

# Verify everything compiles, lints, and passes tests
just verify
```

`just verify` is the primary gate: it runs `oss-boundary-check-strict`, `fmt-check`, `lint`, and `test` in that order. It must pass before every commit.

### Surface-specific gates

Run the narrowest gate that covers what you changed. Do not skip the gate for "small" changes.

| What changed | Gate to run |
|---|---|
| Rust (any workspace crate) | `just verify` |
| Leptos UI (`hypercolor-ui`) | `just ui-test && just ui-build` |
| TypeScript SDK | `just sdk-lint && just sdk-check && just sdk-build` |
| Python client | `just python-verify` |
| Device database (`data/drivers/vendors/`) | `just compat-check` |
| Documentation | `just docs-build` |
| Dependencies or licenses | `just deny` |
| OpenAPI schema | `just python-generate-check` |

{% callout(type="warning") %}
`cargo check --workspace` does NOT cover `hypercolor-ui`. That crate is excluded from the Cargo workspace and must be checked separately with `just ui-test` or `just ui-build`.
{% end %}

---

## Build commands

| Command | What it does |
|---|---|
| `just build` | Debug build, all workspace crates |
| `just build-preview` | Preview profile — runtime-tuned, no debug assertions |
| `just release` | Full release bundle with binaries, assets, docs, and agent skills |
| `just check` | Type-check without building |
| `just test` | All workspace tests |
| `just test-crate NAME` | Tests for one crate (e.g. `hypercolor-core`) |
| `just test-one PATTERN` | Tests matching a name pattern |
| `just lint` | Clippy with `-D warnings` across all targets |
| `just lint-fix` | Auto-fix Clippy suggestions |
| `just fmt` | Rustfmt across the workspace |
| `just fmt-check` | Format check without modifying |
| `just verify` | oss-boundary-check + fmt-check + lint + test |

### Running locally

| Command | What it starts |
|---|---|
| `just daemon` | Daemon on `:9420` (preview profile, debug logging) |
| `just daemon-servo` | Daemon with Servo HTML effect rendering enabled |
| `just tui` | TUI client (starts a daemon if none is running) |
| `just tray` | System tray applet |
| `just cli` | The `hypercolor` CLI |
| `just dev` | Daemon (Servo) and UI dev server together |
| `just ui-dev` | Leptos dev server on `:9430`, proxies API to `:9420` |
| `just sdk-dev` | TypeScript SDK dev server with HMR |

---

## Code conventions

### Rust

- **Edition 2024**, Rust 1.94 or later.
- **Clippy pedantic** enforced at deny level. See `Cargo.toml` for the explicit allow-list.
- **`unsafe` is forbidden** workspace-wide. The two audited exceptions are `hypercolor-linux-gpu-interop` and `hypercolor-windows-pawnio`, which operate at the OS/GPU boundary.
- **`unwrap()` is forbidden.** Use `?`, `.ok()`, `expect("clear reason")`, or handle the error explicitly.
- **`thiserror`** for library error types; **`anyhow`** for application (binary) errors.
- **`tracing`** for all logging. Never `println!` in library code.
- **Serde** defaults: `#[serde(rename_all = "snake_case")]` on enums, `#[serde(default)]` for backwards-compatible deserialization.

### Tests

Tests go in the `tests/` directory of each crate, not in inline `#[cfg(test)]` blocks:

```
crates/hypercolor-core/
  src/
    engine.rs
  tests/
    engine_tests.rs      # covers engine.rs
    sampler_tests.rs     # covers the spatial sampler
```

Every public type and function needs test coverage. When adding a feature, add a test. When fixing a bug, add a regression test.

### Comments

Write a comment only when the why is non-obvious: a hidden constraint, a subtle invariant, or a workaround that would surprise a reader. Do not explain what the code does — well-named identifiers do that. Do not reference the current task or caller, and do not add comments that will rot as the code evolves. Those notes belong in the commit message, not the source.

---

## Commit conventions

Use [Conventional Commits](https://www.conventionalcommits.org/). Every commit gets a body — a bare subject line is not enough.

```
feat(hal): add Corsair iCUE protocol driver

Initial support for Corsair's USB HID lighting protocol, covering
Lighting Node Core and LINK hub enumeration. Color frames use the
direct-mode pipeline with per-channel RGB packing.
```

Scopes map to crate short-names (drop the `hypercolor-` prefix):

`types`, `core`, `hal`, `linux-gpu-interop`, `windows-pawnio`, `driver-api`, `driver-builtin`, `driver-hue`, `driver-nanoleaf`, `driver-wled`, `driver-govee`, `network`, `daemon`, `cli`, `tui`, `tray`, `app`, `leptos-ext`, `leptos-ext-macros`, `ui`

Use `sdk` for TypeScript SDK changes, `docs` for documentation, `data` for device database changes.

Subject lines: imperative mood, 76 characters or fewer, no trailing period. Body: plain prose, wrap at 76 characters, explain why not what.

---

## Crate boundaries

The dependency graph has hard rules. Violating them produces circular dependencies that break the workspace.

- **`hypercolor-types`** depends on nothing. All shared vocabulary types live here.
- **`hypercolor-hal`** depends on `types` only. It must never depend on `core`.
- **`hypercolor-core`** depends on `types` and `hal`. Engine logic and traits live here.
- **`hypercolor-daemon`** is the binary; it depends on `core`, `driver-api`, `network`, and `driver-builtin`. Nothing imports it.
- Network drivers (`driver-hue`, `driver-nanoleaf`, `driver-wled`, `driver-govee`) depend on `driver-api`, not on `core` directly.

If a type is needed in more than one crate, it belongs in `hypercolor-types`.

---

## Agent skills 🔮

Domain-specific authoring knowledge lives in `.agents/skills/`. Each skill's `SKILL.md` is the primary reference for its domain; the `references/` subdirectory holds deeper detail.

| Skill | When to use it |
|---|---|
| `hal-driver-development` | USB/HID/SMBus wire-format encoding, `Protocol` trait, `CommandBuffer`, `zerocopy` structs |
| `protocol-research` | USB captures, community docs, writing a protocol spec before implementation |
| `native-effect-authoring` | Rust `EffectRenderer` implementations in `core/src/effect/builtin/` |
| `rgb-effect-design` | LED color science, HTML canvas effects, palette authoring |
| `leptos-ui-development` | Leptos 0.8 signals, binary WebSocket frames, Luminary design tokens |
| `daemon-development` | `AppState`, REST API, event bus, render pipeline internals, MCP server |

Skills are authored for AI agents working inside the codebase, not for human readers. They encode codebase-specific invariants and patterns that are expensive to rediscover. Browse them before starting a non-trivial implementation in their domain.

---

## Specs and design docs

Before implementing any non-trivial module, check `docs/specs/` for a relevant numbered spec. Specs contain design decisions, API contracts, and edge-case constraints that are not always visible from the code.

### Writing a new spec

A good spec follows this structure:

**1. Motivation** — what problem this solves and why now. One or two paragraphs. Reviewers need to agree on the problem before they can evaluate the solution.

**2. Scope** — what is explicitly in and explicitly out. An out-of-scope list is as important as the in-scope list; it prevents scope creep during review and implementation.

**3. Design** — types, traits, API surface, data model. This is the bulk of the document. Prefer actual Rust type sketches over prose descriptions of types.

```rust
// Example: sketch the public surface, not the implementation
pub trait Protocol: Send + Sync {
    fn encode_frame(&self, frame: &[Rgb], buf: &mut CommandBuffer) -> Result<(), EncodeError>;
    fn init_sequence(&self) -> &'static [u8];
}
```

**4. Wire format or protocol** (for driver specs) — byte-by-byte layout with field names, sizes, and byte order. Ambiguity here causes bugs that are hard to catch in testing because they only surface with real hardware.

```
Offset  Size  Field
0       1     report_id  (always 0x00 for HID)
1       1     command    (0x0b = set lighting)
2       1     channel    (0 = head, 1 = logo)
3       3*N   rgb_data   (R G B per LED, row-major)
```

**5. Implementation plan** — ordered steps, crate owners, test strategy. Break the work into pieces that each compile and pass tests independently. A plan that has to land all at once is a plan that blocks main.

**6. Open questions** — unresolved choices that need a decision before or during implementation. Capturing these in the spec surfaces them early, before they become blockers mid-implementation.

### Where specs live

- Implementation specs: `docs/specs/NN-short-name.md` (numbered, e.g. `docs/specs/42-govee-lan.md`)
- Broader design documents: `docs/design/NN-short-name.md`
- Superseded plans and shipped decisions: `docs/archive/`

Write the spec, get review, then implement against it.

---

## Effect contribution path

See [Adding an effect](@/contributing/adding-an-effect.md) for the full walkthrough. The short version:

**TypeScript SDK effects** live in `sdk/src/effects/` and build to self-contained HTML files via `just effect-build NAME`. They run inside Servo's renderer and can use the full HTML canvas API, WebGL2, and the `HypercolorSDK` runtime. Browse existing effects in `sdk/src/effects/` for patterns before writing from scratch. See [Effects](@/effects/_index.md) for the SDK API reference and dev workflow.

**Native Rust effects** implement `EffectRenderer` and register via `register_builtin_effects()` in `crates/hypercolor-core/src/effect/builtin/mod.rs`. Adding an effect means creating a new submodule in that directory, implementing `EffectRenderer`, and adding a `metadata()` constructor that returns `EffectMetadata`. Native effects produce `Canvas` frames entirely in Rust and run at roughly 1 ms per frame without Servo overhead, making them the right choice for performance-critical or audio-reactive work.

{% callout(type="info") %}
There is no GPU shader lane for native Rust effects. `EffectSource::Shader` is not a runnable path. GLSL effects run as WebGL2 inside Servo, not as compiled wgpu shaders. Frame GPU/wgpu texture import is infrastructure for Servo frame delivery, not a general shader pipeline. If you want a GLSL effect, write it as a WebGL2 SDK effect.
{% end %}

---

## Driver contribution path

See [Adding a HAL driver](@/contributing/adding-a-driver.md) and [Adding a network driver](@/contributing/adding-a-network-driver.md).

For HAL drivers, research comes before code. USB captures of the vendor's own software are the ground truth. Community protocol docs and open-source RGB projects (liquidctl, openrazer) provide useful context but must be verified against captures. Use the `protocol-research` skill for capture methodology and spec writing, then `hal-driver-development` for implementation.

Driver modules are organized by silicon or OEM family, not by brand. Rebranded SKUs that share the same silicon are model-enum variants within that module, not separate modules.

---

## Pull request process

1. Branch from `main`.
2. Make focused changes — one logical change per PR.
3. Run the appropriate gate (`just verify` at minimum) and confirm it passes.
4. Open a PR with a description that explains what changed and why. Link any relevant spec.
5. Respond to review feedback. Do not squash until a maintainer asks.

{% callout(type="tip") %}
The device compatibility matrix (`docs/content/hardware/compatibility.md`) is generated output. Regenerate it with `just compat` after editing `data/drivers/vendors/*.toml`. Never hand-edit the rows between the BEGIN/END markers.
{% end %}
