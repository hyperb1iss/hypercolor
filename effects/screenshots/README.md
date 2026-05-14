# Effect Screenshots

Capture output for effect card artwork, served by the daemon at
`/api/v1/effects/screenshots/<slug>/<variant>.webp` when curated assets are
installed.

For v0.1.0, Hypercolor does **not** ship curated effect screenshots. The UI
probes this endpoint and falls back to opportunistic local thumbnails, then to a
category-coloured gradient. That keeps the launch repository and release
tarballs small while the screenshot set is still being curated.

## Layout

```
effects/screenshots/
├── curated/              # Gitignored local review output for now
│   └── <slug>/
│       ├── default.webp  # Default-controls capture
│       └── <preset>.webp # Named preset variants
└── drafts/               # Gitignored, capture-tool output awaiting review
    └── <slug>/
        └── <variant>/
            ├── rank-1.png
            ├── rank-2.png
            └── rank-3.png
```

`<slug>` is `kebab-case(effect.name)` — e.g. `color-wave`, `audio-pulse`.
`<variant>` is `default` or `kebab-case(preset.name)` — e.g. `silk-sweep`, `aurora-drift`.

## Workflow

1. Start the daemon (`just daemon`).
2. Run the capture tool: `just capture-screenshots` (or target one effect with
   `bun sdk/scripts/capture-screenshots.ts --effect color-wave`). Output lands in `drafts/`.
3. Review `drafts/<slug>/<variant>/rank-{1,2,3}.png` — each rank comes from an HSV
   score combining mean saturation and luminance variance. Rank 1 is usually the pick.
4. Promote chosen frames to `curated/<slug>/<variant>.webp`. The tool ships with a
   `--promote` flag that re-encodes the rank-1 frame of every variant as WebP q=0.92.
5. Before publishing curated images, add an explicit asset policy and update
   `.gitignore`/release packaging so only the reviewed, size-bounded set ships.

## UI contract

`crates/hypercolor-ui/src/components/effect_card.rs` reads each effect's background as an
`<img>` at `/api/v1/effects/screenshots/<slug>/default.webp`. A missing file surfaces
as a 404 and the card falls back to the opportunistic localStorage thumbnail, then to
a category-coloured radial gradient.

Preset variants aren't shown on cards yet — they exist so we can expand card states
or swap artwork when a preset is active in a future pass.
