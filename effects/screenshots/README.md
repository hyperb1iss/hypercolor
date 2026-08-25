# Effect Screenshots

Capture output for effect card artwork. This directory holds the **capture
workflow's** working files; it is not where shipped artwork lives.

Shipped card art travels inside the effect that owns it. A `cover.webp` beside
an effect's `main.ts` is embedded as a `data:image/webp;base64,` URI in the
built artifact's `<meta cover>` tag, and the daemon decodes it on demand at
`/api/v1/effects/<id>/cover`. That keeps artwork and effect in one file, so a
shared `.html` carries its own card art and a rebuild can never drop it.

Curated files under `curated/` still win when present, which is what makes this
directory useful: it is the local override layer for reviewing replacement art
without rebuilding an effect.

## Layout

```
effects/screenshots/
├── curated/              # Gitignored override layer
│   └── <slug>/
│       ├── default.webp  # Overrides the effect's inline cover
│       └── <preset>.webp # Named preset variants
└── drafts/               # Gitignored, capture-tool output awaiting review
    └── <slug>/
        └── <variant>/
            ├── rank-1.png
            ├── rank-2.png
            └── rank-3.png
```

`<slug>` is `kebab-case(effect.name)` — e.g. `color-wave`, `audio-pulse`.
`<variant>` is `default` or `kebab-case(preset.name)` — e.g. `silk-sweep`.

## Workflow

1. Start the daemon (`just daemon`).
2. Capture: `just capture-screenshots` (or one effect with
   `just capture-screenshots --effect color-wave`). Output lands in `drafts/`.
3. Review `drafts/<slug>/<variant>/rank-{1,2,3}.png` — each rank comes from an
   HSV score combining mean saturation and luminance variance. Rank 1 is
   usually the pick.
4. Install the picks as effect covers with `just sync-covers`, which downscales
   rank-1 to a 960px WebP and writes `sdk/src/effects/<id>/cover.webp`. Pass
   `--force` to replace covers that already exist.
5. Rebuild (`just effects-build`) so the new covers embed, then commit the
   `cover.webp` files.

`sync-covers` prefers a fresh draft over the docs-site imagery, so re-running
the capture tool is what ships.

## Sizing

Covers embed in every built artifact and the daemon parses each one on a
registry scan, so they are deliberately small: 960px wide WebP at q88, which
lands between roughly 4KB and 130KB per effect, with the median near 29KB. The
build warns above 256KB and the parser refuses anything over 1MB.
Full-resolution art for the documentation site lives in
`docs/static/img/effects/`.

## UI contract

`crates/hypercolor-ui/src/components/effect_card.rs` renders the
`cover_image_url` the daemon advertises on each effect. Effects with no cover
omit the field entirely, and the card falls back to an opportunistic
localStorage thumbnail, then to a category-coloured radial gradient.

Preset variants aren't shown on cards yet — they exist so we can expand card
states or swap artwork when a preset is active in a future pass.
