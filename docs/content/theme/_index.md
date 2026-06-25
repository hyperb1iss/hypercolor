+++
title       = "The docs run Luminary"
description = "What Luminary is for the docs, the keep/skip table, and a link to the canonical app design system."
weight      = 100
sort_by     = "weight"
+++

# 💜 The docs run Luminary

![Hypercolor wordmark](/img/brand/lockup-horizontal-480.png)

Luminary is Hypercolor's visual language: a dark scrim built around light, where
restrained chrome lets the RGB effects take the stage. The web UI
(`crates/hypercolor-ui/`) is its primary home, but the docs site draws from the
same system — the same token philosophy, the same typefaces, the same accent
discipline.

This section is the public-facing slice of that story. The full canonical
specification lives in
[`docs/DESIGN-SYSTEM.md`](https://github.com/hyperb1iss/hypercolor/blob/main/docs/DESIGN-SYSTEM.md):
token architecture, component patterns, the elevation model, motion easings,
ambient reactivity, and the §14 rules checklist. Nothing here duplicates it;
this section exists to orient contributors working on the docs site specifically.

---

## What Luminary means for the docs

The docs site is not the app UI. It is a Zola static site with its own Sass
(`docs/sass/`) and HTML templates (`docs/templates/`). It inherits Luminary's
intent and aesthetic without importing the app's Tailwind build or Leptos
components.

In practice that means:

- The same **typeface stack**: Satoshi (sans and display) and JetBrains Mono
  (code), loaded via Bunny Fonts (`fonts.bunny.net`), the same origin the app
  uses. `--font-display` resolves to Satoshi; the Bunny request also pulls Sora
  but it has no mapped token and docs do not use it.
- The same **OKLCH color vocabulary**: `--accent` is `oklch(0.65 0.30 320)`
  (electric purple); surfaces and text ride a low-chroma hue-280 neutral ramp
  while borders are translucent white (`oklch(1 0 0 / α)`); focus rings use cyan
  `oklch(0.88 0.18 175)` in dark mode and purple in light.
- The same **dark-first stance**: dark is the primary mode, light is a fully
  supported complement, and the token swap happens at Tier 2 (semantic tokens)
  only. The `data-theme` attribute on `<html>` defaults to `dark`; the
  `theme-toggle.js` script restores any stored preference and persists changes.
- The same **ambient tinting** concept: `--ambient-hue` has no live canvas to
  sample in a static site, so it holds a static `320` (purple) fallback.
  Scrollbars, edge glows, and border accents all honor it — if a future hero
  shader writes the property, the whole surface reacts automatically.
- The same **elevation-through-luminance** rule: higher surfaces are brighter,
  not deeper. Box-shadow is reserved for glows (energy) and the single
  sanctioned sticky-header exception; it is never used to simulate depth.
- The same **purple-only chrome discipline**: electric purple is the sole
  saturated accent in interface chrome. Category colors and effect-category
  accents appear only where they carry semantic meaning, never as decoration.

The hero shader on the home page, the gradient text in headings, and the
breathing glow behind the hero are all direct translations of Luminary
principles into static CSS. They are the system working as designed.

---

## Keep / skip table

Use this when authoring or reviewing docs-site styles. The question is always:
does this belong in the docs CSS, or does it live in the app and should not be
carried over?

| Element | Status | Notes |
|---|---|---|
| Satoshi + JetBrains Mono via `fonts.bunny.net` | **Keep** | Same CDN as the app; no Google Fonts, no Inter |
| OKLCH semantic token names (`--surface-base`, `--accent`, `--text-secondary`, etc.) | **Keep** | Same intent; docs Sass maintains its own values in `_variables.scss` |
| Purple-only chrome accent (`oklch(0.65 0.30 320)`) | **Keep** | Hard rule; cyan/coral/green are not UI chrome |
| Cyan focus rings only (`oklch(0.88 0.18 175)` in dark, purple in light) | **Keep** | Interactive focus indicator, never decoration |
| Ambient hue tinting on borders and scrollbars | **Keep** | Static `320` fallback; activates fully if a hero shader writes `--ambient-hue` |
| Elevation through luminance (brighter = higher) | **Keep** | Same rule, same rationale |
| Dark default, light as real complement | **Keep** | `data-theme="dark"` on `<html>`; `theme-toggle.js` restores stored preference |
| Sharp radius system (max 8 px, pills at 999 px) | **Keep** | Canonical: `--radius-sm/md/lg/xl` = 2/4/6/8 px; nothing else |
| `--ease-silk` / `--ease-spring` motion easings | **Keep** | Consistent feel across app and docs |
| Noise overlay (`body::after`, SVG turbulence, ~1.4 % dark / 0.6 % light) | **Keep** | Already correct in `_base.scss`; suppressed under `prefers-reduced-motion` |
| Tailwind v4 utility classes from the app | **Skip** | Docs uses Sass; do not import the app's Tailwind build |
| Leptos component classes from `input.css` | **Skip** | App-only; translate the intent into Sass equivalents |
| `tailwind.config.js` | **Skip** | Quick-reference documentation only in the app; irrelevant to Zola/Sass |
| `--ambient-hue` set from Leptos signals | **Skip** | Docs uses a static fallback; never wire in Leptos |
| Per-category glow triplets (cyan/coral/green/blue) | **Skip** | Content/semantic colors only; never reuse as docs chrome |
| `tokens/primitives.css` / `tokens/semantic.css` raw `@import` | **Skip** | Docs maintains its own Sass; do not import app token files directly |
| Glass on primary surfaces (desktop sidebar, page body) | **Skip** | Glass (`.glass`, `backdrop-filter`) is for floating elements only: search modal, mobile drawer |

---

## Font loading

Fonts load from Bunny Fonts, the same privacy-respecting CDN the app uses. The
`<head>` of `docs/templates/base.html` must carry these two `<link>` tags:

```html
<link rel="preconnect" href="https://fonts.bunny.net" />
<link href="https://fonts.bunny.net/css?family=satoshi:400,500,600,700|jetbrains-mono:400,500,600,400i|sora:400,500,600" rel="stylesheet" />
```

This is the exact URL from `crates/hypercolor-ui/index.html`, and it is the
origin the `_variables.scss` font stack (`--font-display`, `--font-sans`)
already assumes. Do not substitute Google Fonts or Inter. Keeping the request in
sync with the app prevents typeface drift that makes the docs feel off-brand
even when the tokens are correct.

---

## In this section

| Page | What it covers |
|---|---|
| [Tokens](@/theme/tokens.md) | The semantic token set available to docs-site Sass: surfaces, text, borders, accent, ambient, status, and motion |
| [Accent discipline](@/theme/accent-discipline.md) | The purple-only chrome rule in detail: what counts as chrome, what counts as content, and the review checklist |
| [Contributing styles](@/theme/contributing.md) | How to add or change docs-site styles: Sass conventions, testing both themes, and the sign-off checklist |

---

## Canonical reference

Everything above is a summary. When in doubt, the source of truth is
`docs/DESIGN-SYSTEM.md` in the repo. That document covers:

- The full philosophy (§1) and system anatomy (§2)
- Complete token tables for Tier 1 primitives and Tier 2 semantics (§3),
  including theme switching and light-mode derivation (§3.3)
- Color and accent discipline, the category color map, and the `--glow-rgb`
  named-accent system (§4)
- Typography scale and weight discipline (§5)
- Surfaces and the elevation-through-luminance model (§6)
- Glass / Prism layer rules — floating elements only (§7)
- Motion system and animation budget (§8)
- Ambient reactivity and the `--ambient-hue` contract (§9)
- The logo and brand-stage treatment (§11)
- The §14 checklist every contributor runs before shipping a UI change

When the guide and the CSS disagree, the CSS wins and the guide is what to fix.
Treat that drift as a bug, not a fork.
