# OSS/Internal Boundary

Hypercolor ships as two layers:

- **OSS Hypercolor** is the public engine, driver ecosystem, local daemon,
  local UI, SDK, effects runtime, hardware compatibility data, and extension
  seams.
- **hypercolor-internal** is the private commercial layer for official builds,
  Hypercolor Cloud, entitlements, update channels, extended UI, private
  packaging, and release composition.

The dependency direction is one-way:

```text
hypercolor-internal -> hypercolor OSS
```

OSS must never depend on the internal repo, private crates, commercial services,
or private build tooling. Internal code may depend on OSS crates through an
explicit `oss.lock` pin.

## Versioned Bridge

The internal repo owns `oss.lock` with this version-1 schema:

```toml
version = 1
oss_path = "../worktrees/hypercolor/nova/internal-split"
oss_branch = "nova/internal-split"
oss_commit = "<full git sha>"
```

Official builds update that pin deliberately. The pin is the cross-repo contract:
internal builds prove which OSS commit they compose with, and OSS changes can be
reviewed without private-code context.

## Ownership

OSS owns:

- local render pipeline, device lifecycle, scenes, profiles, local config, SDK,
  effect authoring, public REST/WebSocket/MCP APIs, and public drivers;
- extension traits, route mounting seams, CLI command registration seams, and
  config migration hooks needed by downstream build layers;
- vendor cloud integrations that are required for public hardware support.

Internal owns:

- Hypercolor Cloud authentication, daemon-link tunneling, cloud sync, account
  state, entitlements, private update channels, commercial packaging, and any
  UI that requires those services;
- official build composition that wires internal implementations into OSS seams;
- read-side migration for legacy public configs that contained Hypercolor Cloud
  keys.

Govee Cloud is a public vendor integration and stays OSS. Boundary guards must
look for Hypercolor Cloud markers, not generic `cloud` strings.

## Extraction Order

1. Keep current cloud feature checks green before moving code.
2. Add OSS seams while the current cloud implementation still compiles.
3. Move Hypercolor Cloud crates, daemon routes, CLI commands, tests, and UI into
   `hypercolor-internal`.
4. Remove OSS `cloud` and `official-cloud` feature flags.
5. Enable the strict OSS boundary guard in normal verification.

This order keeps each side buildable while the split happens and gives bisect a
clean line between "seam added" and "implementation moved."

## Guard Policy

`scripts/check-oss-boundary.sh` has two modes:

- baseline mode: checks the guard itself and documents that strict enforcement
  is intentionally deferred during extraction;
- strict mode: fails if Hypercolor Cloud markers remain in OSS, excluding public
  Govee vendor cloud support and the boundary design doc.

Strict mode is part of `just verify` after the extraction commit removes the
current OSS cloud implementation.
