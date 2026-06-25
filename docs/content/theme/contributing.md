+++
title = "Theming a component"
description = "Add a styled component without drifting: reference tokens not raw oklch, support both themes, honor reduced-motion."
weight = 30
+++

The docs theme is a Luminary port. Every component you add follows the same rules as the app UI: semantic tokens over raw values, purple-only chrome, both themes, motion that can be suppressed. This page is the contributor checklist.

---

## The three-tier model

Tokens in the docs theme mirror the app's three-tier architecture.

**Tier 1 — primitives.** Raw OKLCH values in `crates/hypercolor-ui/tokens/primitives.css`. Never reference these in component CSS. They are constant across themes.

**Tier 2 — semantic.** Intent-mapped tokens in `crates/hypercolor-ui/tokens/semantic.css`, mirrored into `docs/sass/_variables.scss`. Components use these. They swap per theme under `[data-theme="dark"]` and `[data-theme="light"]`.

**Tier 3 — component.** Classes built on Tier 2 in `docs/sass/_components.scss`. In the Leptos app, Tailwind v4 generates utility aliases from `@theme` declarations; in the docs SCSS theme, components call `var(--surface-base)` directly. The discipline is the same either way.

Theme switching happens at Tier 2 only. Tier 1 is constant; Tier 3 is theme-agnostic. When you add a component, stay in Tier 2 and Tier 3.

---

## Core rule: tokens, not raw values

Raw `oklch()` values in component CSS defeat the theme swap. They are hardcoded to one theme and will be wrong in the other.

**Do not do this:**

```scss
.my-panel {
  background: oklch(0.155 0.016 280);      // hardcoded dark void-3
  border: 1px solid oklch(1 0 0 / 0.10);  // hardcoded dark border-subtle
  color: oklch(0.96 0.01 280);             // hardcoded dark text-primary
}
```

**Do this instead:**

```scss
.my-panel {
  background: var(--surface-overlay);
  border: 1px solid var(--border-subtle);
  color: var(--text-primary);
}
```

The semantic tokens resolve correctly in both dark and light. Dark is the default under `:root`; light is `[data-theme="light"]`. Both blocks live in `docs/sass/_variables.scss`.

The full token families:

| Family | Tokens |
|---|---|
| Surfaces | `--surface-base`, `--surface-raised`, `--surface-overlay`, `--surface-sunken`, `--surface-hover`, `--surface-active` |
| Text | `--text-primary`, `--text-secondary`, `--text-tertiary`, `--text-inverse` |
| Borders | `--border-subtle`, `--border-default`, `--border-strong`, `--border-focus` |
| Accent | `--accent`, `--accent-hover`, `--accent-muted`, `--accent-subtle` |
| Glows | `--glow-accent`, `--glow-accent-50`, `--glow-accent-70`, `--glow-accent-80`, `--glow-focus`, `--glow-focus-soft` |
| Ambient | `--ambient-glow`, `--ambient-border`, `--ambient-tint` |
| Status | `--status-success`, `--status-error`, `--status-warning`, `--status-info` |
| Glass | `--glass-bg`, `--glass-bg-dense`, `--glass-border` |

The one narrow exception: `inset 0 1px 0 oklch(1 0 0 / 0.04)` in a card hover state is a white sliver that is intentionally theme-invariant (a white inner highlight reads correctly on any elevated dark or light surface), and no semantic token maps to it. That pattern and only that pattern is permitted to carry a raw value. Everything else routes through Tier 2.

---

## Chrome accent discipline

Electric purple (`--accent`) is the only color allowed on UI chrome. Chrome means any navigational or structural element: nav links, sidebar, TOC active states, buttons, hover borders, focus rings, and anything new you add.

Every other palette color serves a specific semantic purpose:

- **Callouts** use status tokens (`--status-warning`, `--status-error`, `--status-info`) or `--color-cyan` for tips. These encode meaning, not decoration.
- **API method badges** use per-method colors (GET cyan, POST green, PUT yellow, DELETE red, PATCH purple). That encoding mirrors HTTP semantics.
- **Focus rings** are cyan in dark mode, purple in light mode. Both come from `--glow-focus` — it is a token, and it swaps. Cyan focus in dark is the one correct, intended use of cyan on chrome.

If you reach for `--color-cyan` on a navigational or structural element, stop. That is a chrome leak.

**Wrong — cyan on chrome:**

```scss
.nav-link:hover {
  color: var(--color-cyan);
  box-shadow: 0 0 12px var(--color-cyan);
}
```

**Correct — purple chrome:**

```scss
.nav-link:hover {
  color: var(--accent);
  background: var(--accent-subtle);
  box-shadow: 0 0 12px var(--glow-accent-50);
}
```

The full decision table is in [@/theme/accent-discipline.md](@/theme/accent-discipline.md).

---

## Surface and elevation

Depth in Luminary comes from luminance steps and hairline borders, not drop shadows.

Higher elevation means a brighter surface. `--surface-overlay` (cards, panels) is brighter than `--surface-raised` (sidebar, header), which is brighter than `--surface-base` (page background). Dark mode brightens each step up; light mode darkens each step down. Both follow the same rule.

Layers are separated by a 1px `--border-subtle` hairline. `box-shadow` for elevation is not permitted, with one exception: the sticky page header uses a real layered shadow to stay visible over scrolling content. That is the single sanctioned drop shadow in the entire theme. If you find yourself adding a second one, use a luminance step or a border instead.

Glows are energy, not depth. `box-shadow` for a colored bloom is fine.

```scss
// Correct elevation pattern
.my-card {
  background: var(--surface-overlay);
  border: 1px solid var(--border-subtle);
  border-radius: $radius-lg;
  transition:
    border-color var(--duration-medium) var(--ease-smooth),
    box-shadow 0.4s var(--ease-smooth),
    filter var(--duration-medium) var(--ease-smooth);

  // Hover: border brightens + ambient glow. No translateY.
  &:hover {
    border-color: var(--ambient-border);
    filter: brightness(1.06);
    box-shadow:
      0 0 24px var(--ambient-glow),
      inset 0 1px 0 oklch(1 0 0 / 0.04);
  }

  // Press: squish.
  &:active {
    transform: scale(0.97);
  }
}
```

---

## Glass: floating elements only

The `@mixin glass` in `_variables.scss` applies `backdrop-filter: blur(12px) saturate(1.15)`. Real blur is expensive and, when stacked, makes text unreadable. Use it only on elements that float above primary content.

| Element | Treatment |
|---|---|
| Search modal | `@include glass-dense` — heavier blur over `--glass-bg-dense` tint |
| Mobile sidebar drawer | `@include glass` — real blur when it floats over page content |
| Dropdowns, popovers, tooltips | `@include glass` |
| Desktop sidebar | Solid `--surface-raised`. The sidebar is a primary surface, not floating. |
| Cards and panels | Solid `--surface-overlay`. Same reason. |

When in doubt, use the solid surface. Most panels that seem to want glass read better and render faster as solid raised surfaces.

Under `prefers-reduced-motion`, the universal kill-switch in `_animations.scss` strips `backdrop-filter` from everything automatically. Glass components get this for free — no per-component opt-out needed.

---

## Colored glows and `--glow-rgb`

When a component needs a glow driven by a dynamic or category-specific color, use the `--glow-rgb` custom property rather than hardcoding an `rgba()` triplet. Set it with a named accent class on a parent, or inline from a `category_style()` RGB triplet for dynamic category color:

```html
<!-- named accent class -->
<div class="my-widget accent-cyan">...</div>

<!-- or inline for dynamic category color -->
<div class="my-widget" style="--glow-rgb: 128, 255, 234;">...</div>
```

```scss
.my-widget {
  --glow-rgb: 225, 53, 255; // purple fallback

  &:hover {
    box-shadow: 0 0 20px rgba(var(--glow-rgb), 0.35);
  }
}
```

The RGB triplets for each effect category come from `category_style()` in `crates/hypercolor-ui/src/style_utils.rs`. Copy from the source, not from memory.

---

## Motion

Motion communicates an entrance, a state change, or feedback. It is never decorative.

**Easing tokens** (defined in `docs/sass/_variables.scss`, exposed as CSS custom properties):

| Token | Curve | Use |
|---|---|---|
| `--ease-spring` | `cubic-bezier(0.34, 1.56, 0.64, 1)` | Physical interactive elements: buttons, toggles, pressed states |
| `--ease-silk` | `cubic-bezier(0.4, 0, 0.2, 1)` | Transitions and reveals: cards fading in, panels opening |
| `--ease-smooth` | `cubic-bezier(0.25, 0.1, 0.25, 1)` | Ambient state changes: border color shifting on hover |
| `--ease-out` | `cubic-bezier(0, 0, 0.2, 1)` | Directional exits |

**Duration tokens:** `--duration-fast` (120ms), `--duration-normal` (200ms), `--duration-medium` (300ms), `--duration-slow` (400ms).

**Stagger pattern for multi-item lists** — the "one orchestrated cascade per view":

```scss
.my-grid__item {
  opacity: 0;
  animation: fadeInUp 0.4s var(--ease-silk) both;

  @for $i from 1 through 8 {
    &:nth-child(#{$i}) {
      animation-delay: #{($i - 1) * 0.06}s;
    }
  }
}
```

Use this once per page section. Do not add entrance animations to every element on a page.

**Hover: no lift.** Cards and panels do not `translateY` on hover. The pattern is border-brightens plus ambient-glow plus `brightness(1.06)`. Press squishes to `scale(0.97)`. The existing `.feature-card` in `_components.scss` is the reference implementation.

---

## Reduced-motion suppression

Any new continuous or entrance animation must be covered by `prefers-reduced-motion`. The docs theme uses a universal kill-switch in `docs/sass/_animations.scss`:

```scss
@media (prefers-reduced-motion: reduce) {
  *,
  *::before,
  *::after {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0.01ms !important;
    backdrop-filter: none !important;
    -webkit-backdrop-filter: none !important;
  }

  body::after {
    display: none; // noise overlay
  }
}
```

The universal selector covers every keyframe automatically. You do not need a per-animation opt-out. If your component sets `backdrop-filter` on a sub-element that the universal rule might miss, add it to this block explicitly — but that should be rare.

Glass blur is also stripped here (`backdrop-filter: none !important`). Any component using `@include glass` gets that for free.

---

## Adding a new token

If your component needs a value that the current token set does not cover, add it to both theme blocks in `docs/sass/_variables.scss`. Never define a token only in dark or only in light — a missing light value falls through to the dark value and breaks theme parity.

1. Add to `:root` (dark mode default) with an OKLCH value appropriate for a near-black surface.
2. Add to `[data-theme="light"]` with the light-adapted value. For surfaces, higher lightness. For status colors, lower lightness for contrast on bright backgrounds (follow the pattern of `--status-success: oklch(0.55 0.18 155)` in light).
3. Reference it as `var(--my-new-token)` in the component. Never inline the raw value.
4. Do not add anything to `tailwind.config.js` — that file is documentation-only in the Leptos app (Tailwind v4 ignores it), and the docs theme does not use Tailwind at all.

---

## Spacing and radii

Use the SCSS variables, not arbitrary values.

**Radii:** `$radius-sm` (2px), `$radius-md` (4px), `$radius-lg` (6px), `$radius-xl` (8px). For pills, `$radius-pill` (999px). For circles, `50%`. Nothing between these.

Luminary is deliberately sharp: 8px is the largest corner in the product. If you find yourself reaching for 12px or 16px, check whether you need a pill shape instead.

**Spacing:** `--spacing-xs` (4px), `--spacing-sm` (8px), `--spacing-md` (16px), `--spacing-lg` (24px), `--spacing-xl` (32px). In docs SCSS, components use these as pixel values in padding and gap rules.

---

## Verifying both themes

```bash
cd docs
zola serve
```

Then toggle between dark and light via the theme button in the nav. Check:

- Every surface reads as the correct elevation step.
- Text contrast holds in light mode. Muted labels legible on near-black can drop too low on near-white.
- Status colors (callout borders, API badge text) are visible. The `--status-*` tokens darken in light mode by design; inline OKLCH values do not adapt.
- Glows are subtle, not overwhelming. Light mode glow tokens carry lower opacity; raw `rgba()` values do not adapt.
- Focus rings are visible on every focusable element. The glow ring is cyan in dark, purple in light — both from `--glow-focus`.

Never call a component done in dark mode only.

---

## Component checklist

Before submitting a new component to `docs/sass/_components.scss`:

1. No raw `oklch()` or `#hex` values in component CSS, except the one inset-white-sliver hover pattern noted above.
2. Every color, border, shadow, and glow goes through a Tier 2 semantic token or `--glow-rgb`.
3. Purple (`--accent`) is the only chrome accent. Non-purple colors appear only on status, data, or semantic elements.
4. No `box-shadow` for elevation. The sticky page header is the only exception in the entire theme.
5. Glass (`backdrop-filter` blur) only on floating elements. Primary surfaces are solid.
6. Hover: border brightens + ambient glow + `brightness(1.06)`. No `translateY`. Press squishes to `scale(0.97)`.
7. Radii come from the SCSS `$radius-*` variables. Pills use `$radius-pill`. Nothing else.
8. Every entrance or continuous animation is covered by the `prefers-reduced-motion` block in `_animations.scss`.
9. Every new token is defined in both `:root` (dark) and `[data-theme="light"]`.
10. Verified visually in both `[data-theme="dark"]` and `[data-theme="light"]`. Never assume dark.

The canonical authority for all of these rules is `docs/DESIGN-SYSTEM.md §14`. When this page and that document disagree, the design system document wins.
