# Hypercolor brand assets

Single source of truth for the visual identity. App icons, installer art,
website OG images, social — everything derives from here.

## Layout

```
assets/brand/
├── source/      raw AI-generated PNGs (gitignored; mirror of ~/Pictures/hypercolor)
├── master/      tight-cropped, alpha-correct, native-res masters (checked in)
├── mask/        grayscale luminance masks for dynamic UI tinting (checked in)
├── derived/     sized/format variants for app/web/installer (gitignored; built)
└── build.py     orchestrates master + mask + derived generation
```

## Rebuilding

```bash
uv run assets/brand/build.py
```

Reads `source/` and `master/` (depending on stage), writes `master/`, `mask/`,
and `derived/`. Idempotent.

## What lives where

- **`master/`** — the canonical artwork. Edit only by replacing a `source/`
  input and re-running build. Anything that needs a "hypercolor logo" should
  read from here.
- **`mask/`** — grayscale masks for dynamically tinting the logo at runtime
  in the UI. See `mask/README.md` for the three flavors of dynamic-tint and
  Leptos usage.
- **`derived/`** — final sized/format outputs: Tauri app icon set, Windows ICO,
  favicons, OG images, WiX/NSIS installer BMPs, social avatars. Regenerated on
  release, not edited by hand. The Windows installer assets are also mirrored
  into `crates/hypercolor-app/icons/` because Tauri consumes them from the app
  crate during bundling.

## Color palette

Brand triad lives in code as well — see `crates/hypercolor-ui/src/theme.rs`
once wired up. For reference:

| Token | Hex | Used for |
|---|---|---|
| `electric-magenta` | `#e135ff` | top petal, primary accent, "effects" |
| `neon-cyan` | `#80ffea` | lower-left petal, "devices/connection" |
| `coral-pink` | `#ff6ac1` | lower-right petal, "scenes/state" |
| `success-green` | `#50fa7b` | success states only |
| `electric-yellow` | `#f1fa8c` | warnings, attention |
| `void-black` | `#0a0612` | brand bg (dark navy-purple, not pure black) |
