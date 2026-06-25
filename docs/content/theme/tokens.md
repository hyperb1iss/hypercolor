+++
title = "Token system"
description = "Tier 1/Tier 2 OKLCH tables for both themes and the drift-from-canonical reconciliation notes."
weight = 10
+++

The Hypercolor docs theme runs **Luminary** — the same design system as the web UI, ported to Zola/SCSS. Tokens are the contract that makes a theme swap a single attribute flip and keeps both dark and light modes internally consistent.

The authoritative source of token values is the CSS, not this page. When this page and the CSS disagree, fix this page.

```
crates/hypercolor-ui/tokens/primitives.css   Tier 1: raw OKLCH primitives
crates/hypercolor-ui/tokens/semantic.css     Tier 2: intent-mapped, swapped per theme
docs/sass/_variables.scss                    Docs theme port of Tier 1 + Tier 2
```

---

## Architecture

Luminary uses three tiers. Only Tier 2 changes per theme.

```
Tier 1  Primitive    Raw OKLCH values. Constant across themes.
                     Declared under @theme in primitives.css (app) or as SCSS
                     variables + CSS custom properties in _variables.scss (docs).
                     Components never reference these directly.

Tier 2  Semantic     Intent-mapped tokens. Swapped per theme via [data-theme].
                     Components reference these — var(--surface-base) not var(--color-void-1).

Tier 3  Utilities    Tailwind utility classes in the app (bg-surface-base, text-fg-primary).
                     In the docs theme, components reference Tier 2 CSS vars directly;
                     there is no Tailwind build step.
```

Theme switching is a single attribute on `<html>`: `data-theme="dark"` (default) or `data-theme="light"`. The docs theme stores the preference in `localStorage` under the key `theme`. The app uses `hc-theme` — these are not shared.

---

## 💜 Tier 1: Primitives

### Neutral scales

Two six-step ramps, both carrying a violet undertone (hue 280) so the chrome never reads as dead gray. `void` serves dark surfaces; `cloud` serves light.

Chroma falls as lightness rises — deep surfaces carry more violet, bright surfaces almost none. This is intentional: the dark UI reads rich, the light UI reads clean.

| Step | `--color-void-N` (dark) | `--color-cloud-N` (light) |
|------|-------------------------|---------------------------|
| 1 | `oklch(0.110 0.020 280)` | `oklch(0.985 0.005 280)` |
| 2 | `oklch(0.130 0.018 280)` | `oklch(0.960 0.008 280)` |
| 3 | `oklch(0.155 0.016 280)` | `oklch(0.930 0.010 280)` |
| 4 | `oklch(0.185 0.014 280)` | `oklch(0.890 0.012 280)` |
| 5 | `oklch(0.220 0.012 280)` | `oklch(0.840 0.014 280)` |
| 6 | `oklch(0.280 0.010 280)` | `oklch(0.780 0.016 280)` |

### SilkCircuit accent palette

These are raw named primitives. In the UI they are consumed through semantic tokens (`--accent`, `--status-*`), never directly. The legacy hex aliases (`--color-electric-purple` etc.) remain in `primitives.css` for migration safety; do not use them in new code.

| Token | OKLCH | Role |
|-------|-------|------|
| `--color-purple` | `oklch(0.65 0.30 320)` | Primary accent |
| `--color-purple-hover` | `oklch(0.70 0.30 320)` | Accent hover |
| `--color-purple-light` | `oklch(0.58 0.28 320)` | Light-mode accent base |
| `--color-cyan` | `oklch(0.88 0.18 175)` | Interactive focus (dark mode) |
| `--color-coral` | `oklch(0.72 0.22 350)` | Secondary semantic / display-face chrome |
| `--color-yellow` | `oklch(0.93 0.15 105)` | Warnings, attention |
| `--color-green` | `oklch(0.85 0.22 155)` | Success |
| `--color-red` | `oklch(0.68 0.22 25)` | Error, danger |
| `--color-blue` | `oklch(0.72 0.12 260)` | Info |

### Motion

Easings and durations from `primitives.css`. The docs `_variables.scss` also defines `--ease-smooth` (`cubic-bezier(0.25, 0.1, 0.25, 1)`) and `--duration-medium` (`300ms`) as SCSS-only local additions; these have no counterpart in the canonical app tokens.

| Token | Value | Source |
|-------|-------|--------|
| `--ease-silk` | `cubic-bezier(0.4, 0, 0.2, 1)` | canonical |
| `--ease-spring` | `cubic-bezier(0.34, 1.56, 0.64, 1)` | canonical |
| `--ease-out` | `cubic-bezier(0, 0, 0.2, 1)` | canonical |
| `--ease-smooth` | `cubic-bezier(0.25, 0.1, 0.25, 1)` | docs only |
| `--duration-fast` | `120ms` | canonical |
| `--duration-normal` | `200ms` | canonical |
| `--duration-medium` | `300ms` | docs only |
| `--duration-slow` | `400ms` | canonical |

### Radii

Luminary is **sharp**. 8px is the largest corner in the product. Pills use `999px`; circles use `50%`. Nothing else.

| Token | Value |
|-------|-------|
| `--radius-sm` | `2px` |
| `--radius-md` | `4px` |
| `--radius-lg` | `6px` |
| `--radius-xl` | `8px` |
| `--radius-pill` | `999px` |

### Spacing

| Token | Value |
|-------|-------|
| `--spacing-xs` | `4px` |
| `--spacing-sm` | `8px` |
| `--spacing-md` | `16px` |
| `--spacing-lg` | `24px` |
| `--spacing-xl` | `32px` |

### Typography

Fonts load from Bunny Fonts (privacy-respecting, no Google tracking).

| Token | Stack |
|-------|-------|
| `--font-sans` | `'Satoshi', 'Inter', system-ui, -apple-system, sans-serif` |
| `--font-mono` | `'JetBrains Mono', 'Fira Code', 'SF Mono', ui-monospace, monospace` |
| `--font-display` | `'Satoshi', system-ui, sans-serif` |

Base font size is `112.5%` (18px). Mono runs with ligatures off (`font-feature-settings: 'liga' 0`). Sans uses stylistic sets `cv02 cv03 cv04 cv11` globally.

---

## Tier 2: Semantic tokens

Tier 2 tokens are the ones components actually use. Each is defined in both theme blocks — that symmetry is what makes a theme swap a single attribute flip rather than a re-skin.

### Surfaces (both themes)

Surfaces map the neutral ramps to elevation intent. In dark mode each step is brighter than the last; in light mode each step is darker.

| Token | Dark | Light | Use |
|-------|------|-------|-----|
| `--surface-base` | `oklch(0.110 0.020 280)` | `oklch(0.985 0.005 280)` | Page background |
| `--surface-raised` | `oklch(0.130 0.018 280)` | `oklch(0.960 0.008 280)` | Sidebar, header |
| `--surface-overlay` | `oklch(0.155 0.016 280)` | `white` | Cards, panels |
| `--surface-sunken` | `oklch(0.185 0.014 280)` | `oklch(0.930 0.010 280)` | Inputs, wells |
| `--surface-hover` | `oklch(0.220 0.012 280)` | `oklch(0.890 0.012 280)` | Hover states |
| `--surface-active` | `oklch(0.280 0.010 280)` | `oklch(0.840 0.014 280)` | Pressed, selected |

### Text (both themes)

Never pure white or pure black — always a violet-tinted near value.

| Token | Dark | Light |
|-------|------|-------|
| `--text-primary` | `oklch(0.96 0.01 280)` | `oklch(0.18 0.02 280)` |
| `--text-secondary` | `oklch(0.68 0.03 280)` | `oklch(0.42 0.03 280)` |
| `--text-tertiary` | `oklch(0.52 0.04 280)` | `oklch(0.58 0.02 280)` |
| `--text-inverse` | `oklch(0.110 0.020 280)` | `oklch(0.985 0.005 280)` |

### Borders (both themes)

Dark borders are white at three opacity levels; light borders are black at three levels. Note the asymmetry at `--border-strong`: dark uses 22%, light uses 24%.

| Token | Dark | Light |
|-------|------|-------|
| `--border-subtle` | `oklch(1 0 0 / 0.10)` | `oklch(0 0 0 / 0.10)` |
| `--border-default` | `oklch(1 0 0 / 0.16)` | `oklch(0 0 0 / 0.16)` |
| `--border-strong` | `oklch(1 0 0 / 0.22)` | `oklch(0 0 0 / 0.24)` |
| `--border-focus` | `oklch(0.88 0.18 175 / 0.40)` (cyan) | `oklch(0.65 0.30 320 / 0.50)` (purple) |

Focus color flips by theme: **cyan in dark, purple in light**. This is intentional and correct.

### Accent (both themes)

Light mode desaturates the accent slightly so it does not vibrate against bright surfaces.

| Token | Dark | Light |
|-------|------|-------|
| `--accent` | `oklch(0.65 0.30 320)` | `oklch(0.58 0.28 320)` |
| `--accent-hover` | `oklch(0.70 0.30 320)` | `oklch(0.52 0.28 320)` |
| `--accent-muted` | `oklch(0.65 0.30 320 / 0.12)` | `oklch(0.58 0.28 320 / 0.10)` |
| `--accent-subtle` | `oklch(0.65 0.30 320 / 0.06)` | `oklch(0.58 0.28 320 / 0.05)` |

### Status (both themes)

Status tokens are darkened in light mode to maintain contrast on bright surfaces.

| Token | Dark | Light |
|-------|------|-------|
| `--status-success` | `oklch(0.85 0.22 155)` | `oklch(0.55 0.18 155)` |
| `--status-error` | `oklch(0.68 0.22 25)` | `oklch(0.55 0.22 25)` |
| `--status-warning` | `oklch(0.93 0.15 105)` | `oklch(0.58 0.14 85)` |
| `--status-info` | `oklch(0.72 0.12 260)` | `oklch(0.50 0.12 260)` |

### Glass (Prism)

Glass tokens are for floating elements only: the search modal and the mobile sidebar drawer. Do not apply `backdrop-filter` to primary surfaces.

| Token | Dark | Light |
|-------|------|-------|
| `--glass-bg` | `oklch(0.13 0.02 280 / 0.70)` | `oklch(0.98 0.005 280 / 0.70)` |
| `--glass-bg-dense` | `oklch(0.13 0.02 280 / 0.85)` | `oklch(0.98 0.005 280 / 0.85)` |
| `--glass-blur` | `12px` | `12px` |
| `--glass-saturate` | `1.15` | `1.15` |
| `--glass-border` | `oklch(1 0 0 / 0.06)` | `oklch(0 0 0 / 0.08)` |

### Glow alpha ladder

The glow ladder provides alpha variants for accent glows without requiring inline `rgba()` values. Both themes define the full ladder; the base values differ to match the desaturated light accent.

| Token | Dark | Light |
|-------|------|-------|
| `--glow-accent` | `oklch(0.65 0.30 320)` | `oklch(0.58 0.28 320)` |
| `--glow-accent-50` | `oklch(0.65 0.30 320 / 0.5)` | `oklch(0.58 0.28 320 / 0.5)` |
| `--glow-accent-70` | `oklch(0.65 0.30 320 / 0.7)` | `oklch(0.58 0.28 320 / 0.7)` |
| `--glow-accent-80` | `oklch(0.65 0.30 320 / 0.8)` | `oklch(0.58 0.28 320 / 0.8)` |
| `--glow-focus` | `oklch(0.88 0.18 175 / 0.30)` | `oklch(0.58 0.28 320 / 0.35)` |
| `--glow-focus-soft` | `oklch(0.88 0.18 175 / 0.15)` | `oklch(0.58 0.28 320 / 0.18)` |
| `--selection-bg` | `oklch(0.65 0.30 320 / 0.25)` | `oklch(0.58 0.28 320 / 0.22)` |

### Ambient reactivity

A single `--ambient-hue` property drives borders, scrollbars, and edge glows. In the UI, this value is updated at runtime from the active effect's dominant hue. In the docs site it stays at the static fallback of `320` (purple), which keeps all dependent tokens on-discipline without any live source.

| Token | Dark | Light |
|-------|------|-------|
| `--ambient-hue` | `320` (static fallback) | (inherits from dark) |
| `--ambient-glow` | `oklch(0.65 0.20 var(--ambient-hue) / 0.08)` | `oklch(0.58 0.15 var(--ambient-hue) / 0.06)` |
| `--ambient-border` | `oklch(0.65 0.20 var(--ambient-hue) / 0.15)` | `oklch(0.58 0.15 var(--ambient-hue) / 0.12)` |
| `--ambient-tint` | `oklch(0.65 0.20 var(--ambient-hue) / 0.04)` | `oklch(0.58 0.15 var(--ambient-hue) / 0.03)` |
| `--scrollbar-thumb` | `oklch(0.65 0.10 var(--ambient-hue) / 0.18)` | `oklch(0 0 0 / 0.12)` |
| `--scrollbar-hover` | `oklch(0.65 0.12 var(--ambient-hue) / 0.32)` | `oklch(0 0 0 / 0.24)` |

### Noise

A near-invisible fractal SVG overlay on `body::after` adds tactile depth without images. Opacity drops in light mode and is removed entirely under `prefers-reduced-motion`.

| Token | Dark | Light |
|-------|------|-------|
| `--noise-opacity` | `0.014` | `0.006` |

### Brand stage

The canonical Hypercolor surface treatment for the hero. A deep magenta-velvet background that makes the brand mark read as "lit in a real room."

| Token | Dark | Light |
|-------|------|-------|
| `--brand-stage-inner` | `oklch(0.250 0.130 295)` | `oklch(0.92 0.06 300)` |
| `--brand-stage-outer` | `oklch(0.100 0.020 285)` | `oklch(0.985 0.005 280)` |

`--gradient-brand-stage` composes these as `radial-gradient(ellipse at center top, var(--brand-stage-inner) 0%, var(--brand-stage-outer) 70%)`.

---

## Reconciliation notes: docs vs canonical

The docs theme (`docs/sass/_variables.scss`) is a port of Luminary into Zola/SCSS. Most tokens are byte-correct against `tokens/semantic.css`. The drifts below were present at the time of the last reconciliation; treat any remaining mismatch as a bug in the docs theme, not the canonical tokens.

### Already matching (confirmed in code)

Surfaces (both themes), text (both themes), accent (both themes), glass bg/border, noise opacity, and status (dark) are byte-correct against `tokens/semantic.css`. The SilkCircuit primitives match `tokens/primitives.css`; note the docs light block additionally darkens `--color-cyan` and `--color-coral` for contrast, where the app keeps Tier 1 invariant.

### Known drift areas

**Radii.** The canonical values are `2/4/6/8px`. An earlier docs theme used `6/10/14/20px`. The `_variables.scss` now reflects the canonical values — if you see softer corners on any component, the component is applying its own radius rather than reading the token.

**Border opacities.** Dark mode: white at 10/16/22%. Light mode: black at 10/16/24%. Earlier versions had 6/10/16 (dark) and 6/10/18 (light) — both too faint. The canonical CSS is the source of truth.

**Light-mode status tokens.** The light block must define all four status tokens with darkened values for contrast on bright surfaces. If only the dark values are defined, light-mode callouts inherit values that fail contrast requirements.

**Inline hardcoded `oklch()` in component SCSS.** Any `oklch()` value in `docs/sass/*.scss` that is not behind a `var()` does not swap with the theme. These should be routed through the appropriate Tier 2 token. The most common leak is cyan `oklch(0.88 0.18 175)` appearing in component rules — see [`@/theme/accent-discipline.md`](@/theme/accent-discipline.md) for the full audit checklist.

**Storage key separation.** The app uses `hc-theme` in `localStorage`; the docs site uses `theme`. Do not claim they share state — they do not.

---

## Using tokens in the docs theme

Reference Tier 2 tokens via `var()` in SCSS partials. Never inline a raw `oklch()` value in a component rule — it will not swap with the theme and will create a parity regression.

```scss
// Correct
.my-component {
  background: var(--surface-raised);
  border: 1px solid var(--border-subtle);
  color: var(--text-primary);
}

// Wrong — does not swap with theme, hardcodes dark values
.my-component {
  background: oklch(0.130 0.018 280);
  border: 1px solid oklch(1 0 0 / 0.10);
  color: oklch(0.96 0.01 280);
}
```

For glow effects, use the alpha ladder rather than constructing inline `oklch()` with alpha:

```scss
// Correct
.my-card:hover {
  box-shadow: 0 0 12px var(--glow-accent-50);
}

// Wrong
.my-card:hover {
  box-shadow: 0 0 12px oklch(0.65 0.30 320 / 0.5);
}
```

Every new component must be tested in both themes before merge. See the contributing guide at [`@/theme/contributing.md`](@/theme/contributing.md) for the verification checklist.

---

## Related

- [`@/theme/accent-discipline.md`](@/theme/accent-discipline.md) — the purple-only chrome rule, de-cyan audit, and semantic vs chrome decision table
- [`@/theme/contributing.md`](@/theme/contributing.md) — how to add a styled component without drifting from canonical
- [`@/theme/_index.md`](@/theme/_index.md) — overview of the docs Luminary port
- Canonical source: `docs/DESIGN-SYSTEM.md` in the repository root (the design intent guide; when it conflicts with the CSS, fix the guide)
