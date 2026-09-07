# Luminary Design System

Hypercolor's visual language: a cinematic control surface that lives and
breathes light.

**Codename:** Luminary (ambient reactivity), Prism (layered glass)
**Status:** Shipped. This is the canonical visual language for every Hypercolor
surface, not a proposal.
**Scope:** The web UI (`crates/hypercolor-ui/`) and any future surface that
renders application chrome.
**Last reconciled with code:** 2026-08-25

> **Source of truth.** Exact token values live in
> `crates/hypercolor-ui/tokens/primitives.css` and `tokens/semantic.css`;
> component styles live in `crates/hypercolor-ui/input.css`. This guide explains
> the *system*: the intent, the rules, how the pieces fit. When the guide and
> the CSS disagree, the CSS wins and this guide is the thing to fix. Treat that
> drift as a bug, not a fork.

Where [`docs/design/01-ux-philosophy.md`](design/01-ux-philosophy.md) defines how
Hypercolor *behaves* (interaction model, personas, workflows), Luminary defines
how it *looks*. The two are complementary; neither overrides the other.

---

## 1. Philosophy

The UI is a **dark scrim around light**. Inspired by Robert Irwin's theatrical
scrims and VJ lighting consoles, the interface defers to the RGB effects it
controls. Restrained chrome provides the stage; the effects, previewed live on
canvas, provide the drama.

Four principles govern every decision in this system.

**1. The light is the star.** UI chrome is neutral and recessive. Saturated
color enters the interface through three doors only: live effect previews,
category semantics, and ambient reactivity. It does not enter through decorative
UI elements. If a surface competes with the canvas for attention, the surface is
wrong.

**2. Depth through luminance, not shadow.** Higher elevation means a brighter
surface, not a darker drop shadow. On a near-black canvas there is nowhere
darker to go. Layers separate through luminance steps and hairline borders.
Box-shadow is reserved for two jobs: the single page-header elevation exception
(§6.3) and colored *glows*, which communicate energy rather than depth.

**3. The UI breathes.** A single `--ambient-hue` custom property, derived from
the active effect's dominant hue, tints borders, scrollbars, and edge glows. The
interface subtly mirrors the light it orchestrates. It is alive without being
busy.

**4. Drama is rationed.** Restraint is the default; cinematic moments are
deliberate and rare. One orchestrated entrance per navigation. One ignition
flourish per effect swap. One wildly expressive logo, quarantined to the
sidebar. Spend drama where it lands; never spray it across every hover.

---

## 2. System Anatomy

Luminary is a CSS-first token system processed by Tailwind v4. There are three
files and one build hook.

```
crates/hypercolor-ui/
  tokens/
    primitives.css   Tier 1: raw OKLCH values + @theme registration
    semantic.css     Tier 2: intent-mapped tokens, swapped per theme
  input.css          Tier 3 component styles, utilities, animations, logo
  tailwind.config.js Reference documentation only; Tailwind v4 ignores it
```

The Trunk pre-build hook runs the Tailwind v4 CLI over `input.css` (which
`@import`s both token files) and emits `tailwind-out.css`. Every custom property
declared under `@theme` in `primitives.css` auto-generates Tailwind utility
classes: declare `--color-surface-base` and `bg-surface-base` exists.

`tailwind.config.js` is **quick-reference documentation, not configuration.**
Tailwind v4 ignores it unless a CSS file opts in with `@config`, and none does.
Never wire a token there expecting it to work; put it in `tokens/`.

### The three tiers

```
Tier 1  Primitive   Raw OKLCH values. Never referenced by components.
Tier 2  Semantic    Intent-mapped tokens. Swapped per theme. Components use these.
Tier 3  Component   Tailwind aliases and component classes built on Tier 2.
```

Theme switching happens at **Tier 2 only**. Tier 1 is constant; Tier 3 is
theme-agnostic. This is the discipline that keeps light mode free.

---

## 3. Token Architecture

### 3.1 Tier 1: Primitives

Raw values in OKLCH, declared under `@theme` in `primitives.css`. OKLCH is
non-negotiable: its perceptual uniformity makes contrast predictable and lets
light mode be derived rather than hand-tuned.

**Neutrals.** Two scales, both carrying a violet undertone (hue 280) so the
chrome never reads as dead gray. `void` is the dark-mode surface ramp; `cloud`
is the light-mode ramp.

| Step | `void` (dark) | `cloud` (light) |
| ---- | ------------- | --------------- |
| 1 | `oklch(0.110 0.020 280)` (deepest) | `oklch(0.985 0.005 280)` (brightest) |
| 2 | `oklch(0.130 0.018 280)` | `oklch(0.960 0.008 280)` |
| 3 | `oklch(0.155 0.016 280)` | `oklch(0.930 0.010 280)` |
| 4 | `oklch(0.185 0.014 280)` | `oklch(0.890 0.012 280)` |
| 5 | `oklch(0.220 0.012 280)` | `oklch(0.840 0.014 280)` |
| 6 | `oklch(0.280 0.010 280)` | `oklch(0.780 0.016 280)` |

Chroma falls as lightness rises: deep surfaces carry more violet, bright
surfaces nearly none. This keeps the dark UI rich and the light UI clean.

**SilkCircuit accents.** The palette is shared across Bliss's tooling ecosystem.
Interactive chrome and status components consume it through semantic tokens.

| Token | OKLCH | SilkCircuit source hex | Role |
| ----- | ----- | ---------------------- | ---- |
| `--color-purple` | `oklch(0.65 0.30 320)` | `#e135ff` | Primary accent |
| `--color-cyan` | `oklch(0.88 0.18 175)` | `#80ffea` | Interactive focus |
| `--color-coral` | `oklch(0.72 0.22 350)` | `#ff6ac1` | Secondary accent |
| `--color-yellow` | `oklch(0.93 0.15 105)` | `#f1fa8c` | Warnings, attention |
| `--color-green` | `oklch(0.85 0.22 155)` | `#50fa7b` | Success |
| `--color-red` | `oklch(0.68 0.22 25)` | `#ff6363` | Error, danger |
| `--color-blue` | `oklch(0.72 0.12 260)` | `#82aaff` | Info |

The hex column is the palette's SilkCircuit lineage value and the exact numeric
triplet the `--glow-rgb` system uses (§4.2). It is **not** a preview of the
OKLCH beside it. The tokens were re-tuned from those hexes and none of the
seven round-trip: purple renders `#d91dfc`, cyan `#00fdd1`, coral `#ff5cb8`,
yellow `#f7ed6c`, green `#00f68b`, red `#ff4c4d`, blue `#78a5ef`. Cyan, coral,
green, and red sit outside sRGB and clip on the way out, cyan worst of all.
Read the two columns as two related systems, never as one value written twice.

`--color-purple-hover` and `--color-purple-light` extend the primary accent.
Data visualization and decorative spectra may use a primitive directly when
the hue itself distinguishes one series from another.

**Other primitives.**

- **Type:** `--font-sans` (Satoshi, falling back to Inter then system),
  `--font-mono` (JetBrains Mono, falling back to Fira Code then SF Mono),
  `--font-display` (Satoshi).
- **Motion easings:** `--ease-silk` `cubic-bezier(0.4, 0, 0.2, 1)`,
  `--ease-spring` `cubic-bezier(0.34, 1.56, 0.64, 1)`, `--ease-out`
  `cubic-bezier(0, 0, 0.2, 1)`.
- **Motion durations:** `--duration-fast` 120ms, `--duration-normal` 200ms,
  `--duration-slow` 400ms.
- **Radii:** `--radius-sm` 2px, `--radius-md` 4px, `--radius-lg` 6px,
  `--radius-xl` 8px. The system is **sharp**: 8px is the largest corner in the
  product. Pills use `999px`; circles use `50%`. Nothing else.
- **Spacing:** `--spacing-xs` 4px, `--spacing-sm` 8px, `--spacing-md` 16px,
  `--spacing-lg` 24px, `--spacing-xl` 32px.

### 3.2 Tier 2: Semantic Tokens

Intent-mapped tokens in `semantic.css`. Components reference these, never Tier 1.
Dark mode lives under `:root, [data-theme="dark"]`; light under
`[data-theme="light"]`. Each token is defined in both blocks, and that symmetry
is what makes a theme swap a single attribute flip.

**Surfaces** map the neutral ramps to elevation intent. In dark mode each step is
brighter than the last; in light mode each step is darker.

| Token | Dark | Light | Use |
| ----- | ---- | ----- | --- |
| `--surface-base` | `void-1` | `cloud-1` | Page background |
| `--surface-raised` | `void-2` | `cloud-2` | Sidebar, header |
| `--surface-overlay` | `void-3` | `white` | Cards, panels |
| `--surface-sunken` | `void-4` | `cloud-3` | Inputs, wells |
| `--surface-hover` | `void-5` | `cloud-4` | Hover states |
| `--surface-active` | `void-6` | `cloud-5` | Pressed, selected |

**Text:** `--text-primary` (near-white in dark, near-black in light, never pure
either way), `--text-secondary` (muted labels), `--text-tertiary` (placeholders,
disabled), `--text-inverse`.

**Borders:** `--border-subtle`, `--border-default`, `--border-strong`,
`--border-focus`. Dark-mode borders are white at 10%, 16%, and 22% opacity;
light mode is black at 10%, 16%, and 24%. `--border-focus` is cyan in dark,
purple in light.

**Accent:** `--accent`, `--accent-hover`, `--accent-muted`, `--accent-subtle`.
The two washes differ per theme: 12% and 6% in dark, 10% and 5% in light. Light
mode also desaturates and darkens the accent so it does not vibrate against
bright surfaces.

**Status:** `--status-success`, `--status-error`, `--status-warning`,
`--status-info` resolve to the green, red, yellow, and blue primitives in dark
mode. Light mode substitutes independently darkened values rather than
re-tinting the same ones, and shifts warning toward amber
(`oklch(0.58 0.14 85)` against the dark `oklch(0.93 0.15 105)`).

Glows and ambient tokens are covered in §4 and §9.

### 3.3 Theme Switching

Theme is a `data-theme` attribute on `<html>` (`dark` is the default and the
unattributed fallback). The preference persists to `localStorage` under
`hc-theme` and is restored by an inline script in `index.html` **before first
paint**, so there is no flash. There is no runtime theme context and no toggle
control: nothing in the Rust UI writes `hc-theme` or sets `data-theme`, so
today light mode is reached only by setting that localStorage key by hand.

Dark is the primary, fully-featured mode. Light is a real, supported complement
for daytime use, not an afterthought, but it is derived from the same system
rather than designed separately. **Every component must work in both.** Test
`[data-theme="light"]` before calling a surface done.

### 3.4 Tier 3: Component Aliases

Tailwind v4 generates a utility class from every `@theme` custom property.
Components use the readable aliases, not raw `var()`.

| Family | Utilities |
| ------ | --------- |
| Surfaces | `bg-surface-{base,raised,overlay,sunken,hover,active}` |
| Text | `text-fg-{primary,secondary,tertiary}` |
| Borders | `border-edge-{subtle,default,strong,focus}` |
| Accent | `{bg,text,border,ring}-accent{,-hover,-muted,-subtle}` |
| Status | `{bg,text,border}-status-{success,error,warning,info}` |
| Palette | `bg-{purple,cyan,coral,yellow,green,red,blue}` |
| Radii | `rounded-{sm,md,lg,xl}` |

Note the deliberate naming: surfaces keep the `surface-` prefix, but text aliases
are `fg-*` and borders are `edge-*`. The short forms avoid the awkward
`text-text-*` and `border-border-*` doubling. Only the families in the table
resolve. There is no bare `text-fg`, no `text-muted`, no `text-dim`, and no
`bg-layer-*`: Tailwind v4 generates a utility only where an `@theme` property
exists, and `primitives.css` declares none of those. Writing one produces a
class that renders nothing.

---

## 4. Color and Accent Discipline

**Electric purple is the only accent in UI chrome.** Buttons, toggles, active
states, slider thumbs, the sidebar active-indicator bar, focus partners: all
purple. A single confident accent is what separates a control surface from a
toy.

Every other SilkCircuit color appears **only** in semantic contexts:

- **Category semantics.** Effect categories carry color (see §4.1).
- **Status.** Success green, error red, warning yellow, info blue.
- **Data and devices.** Visualizations, device backend indicators, per-device
  accents.

If you reach for cyan or coral to decorate a button, stop. That is a category or
status signal leaking into chrome.

### 4.1 Category Color Map

Effect categories map to a badge style and an accent RGB triplet via
`category_style()` in `src/style_utils.rs`, the single source of truth.

| Category | Color | RGB triplet |
| -------- | ----- | ----------- |
| `ambient` | Neon cyan | `128, 255, 234` |
| `audio` | Coral | `255, 106, 193` |
| `generative` | Success green | `80, 250, 123` |
| `particle` | Electric purple | `225, 53, 255` |
| `scenic` | Soft pink (accent hover) | `255, 153, 255` |
| `interactive` | Info blue | `130, 170, 255` |
| `fun` | Light purple | `189, 0, 221` |
| `source` | Electric yellow (status warning) | `241, 250, 140` |
| `utility`, unknown | Tertiary gray | `139, 133, 160` |
| `display` | Coral | `255, 106, 193` |

The triplet feeds the `--glow-rgb` system below. The category set is not open:
it is fixed by the ten `EffectCategory` variants in
`crates/hypercolor-types/src/effect.rs`, and `category_style()` carries exactly
one arm per variant plus a fallback. Adding a category means adding the variant
first, then the arm, then this row.

### 4.2 The `--glow-rgb` Named-Accent System

CSS cannot extract RGB channels from an `oklch()` value, but colored glows and
gradients need numeric channels for `rgba()`. Luminary solves this with a single
custom property, `--glow-rgb`, holding an `R, G, B` triplet.

Component classes (`.edge-glow-accent`, `.card-hover`, `.btn-press`,
`.chip-interactive`, and others) read `--glow-rgb`, defaulting to electric
purple. You set it with a named-accent class instead of spraying raw triplets
through component source:

```
.accent-purple  brand, page headers       .accent-yellow  warnings, timestamps
.accent-cyan    live state, widgets        .accent-green   success states
.accent-coral   display-face chrome        .accent-red     errors
.accent-pink    media library identity
```

For dynamic colors (a preset row painted from its own control values, a card
glowing in its effect's category color), set `--glow-rgb` inline from the
`category_style()` triplet. Never hardcode a raw `rgba(225, 53, 255, ...)` in a
component when a token or `--glow-rgb` will do.

### 4.3 Device Accents

Each device gets a deterministic two-color accent gradient hashed from its ID.
`device_accent_colors()` runs an FNV-1a hash to pick a primary hue, places a
secondary hue `+40°` away, and converts both from HSL to RGB. Same device, same
colors, every session: identity without a stored palette.

---

## 5. Typography

**Satoshi** is the interface typeface, a geometric sans with more personality
than Inter, tighter metrics, and an excellent weight range. It reads as
*designed*. **JetBrains Mono** carries every number, metric, hex value, and code
fragment. **Sora** is the display face and does one job, the page title (§5.2).
All three load from Bunny Fonts (privacy-respecting, no Google tracking), and
they are the only faces the app fetches.

The root is set to `font-size: 112.5%` (an 18px base), so `1rem` is 18px. Body
text runs `letter-spacing: -0.01em` with `line-height: 1.5`; headings tighten to
`-0.02em` and `1.2`. Stylistic sets `cv02 cv03 cv04 cv11` are on globally.

### 5.1 Type Scale

| Level | Size | Weight | Tracking | Use |
| ----- | ---- | ------ | -------- | --- |
| Display | 28px | 700 | -0.03em | Page titles |
| Title | 20px | 600 | -0.02em | Section headers, card names |
| Heading | 16px | 600 | -0.015em | Subsections, panel headers |
| Body | 14px | 400 | -0.01em | Primary content |
| Label | 12px | 500 | 0 | Control labels, nav items |
| Caption | 11px | 500 | 0.01em | Metadata, badges, timestamps |
| Micro | 10px | 500 | 0.02em | Tiny labels, status text |

Mono scale: 14px/500 for hex values in pickers, 12px/400 for status values and
the FPS counter, 10px/400 for slider values and RGB channels. Mono runs with
ligatures off (`liga 0`).

The Display row is the ramp's top step, not a description of the page header.
The shipped page title is `.page-title`: 21px, weight 500, +0.02em, in Sora
(§5.2), and it steps down on the tighter phone header row.

### 5.2 Display Faces

Exactly three families load, all from Bunny Fonts in `index.html`: **Satoshi**
(400/500/600/700), **JetBrains Mono** (400/500/600 plus 400 italic), and
**Sora** (400/500/600). Nothing else is fetched, so any face named in a stack
without one of those three behind it silently falls back.

**Sora** is the display face, and it has exactly one job: `.page-title` in
`input.css`, at 21px, weight 500, +0.02em tracking, with an accent-forward
gradient clipped to the glyphs (a bright tint of the page's accent leading into
the full accent and out to an adjacent-hue tail, plus a dark-only neon halo and
a light-mode re-mix toward ink). `PageHeader` picks the accent through the
`.page-title-*` variants. Do not reach for Sora elsewhere. `--font-display`
still resolves to Satoshi; Sora is applied directly by the `.page-title` rule
rather than through a token.

> **Known remnant.** `crates/hypercolor-ui/src/vendors.rs` returns a stack
> beginning with `'Orbitron'` for `VendorFont::Display`. Orbitron has not
> loaded since the logo modes were removed, so those vendor marks render in
> Satoshi. The stack is a code bug to clean up, not a face to reintroduce.

---

## 6. Surfaces and Elevation

### 6.1 The Layer Model

```
surface-base ─── page background
  surface-raised ─── sidebar, header
    surface-overlay ─── cards, panels
      surface-sunken ─── inputs, wells
```

Dark mode brightens each step; light mode darkens it. Layers separate with a 1px
`--border-subtle` hairline, not a shadow.

### 6.2 Edge Glows

Soft glows stand in for elevation shadows. `.edge-glow` is the neutral panel
treatment: a faint border-tinted ring plus a 1px inner highlight.
`.edge-glow-accent` adds a colored bloom driven by `--glow-rgb` (§4.2).
`.preview-glow` wraps the live canvas in an ambient-reactive halo (§9).
`.modal-glow`, `.dropdown-glow`, and `.swatch-glow` are the floating-element
variants. These glows read as *energy*, not depth.

### 6.3 The One Shadow Exception

`.page-header-elevation` uses real layered `box-shadow` (and a lighter pair in
light mode) to lift the page header above scrolling content. This is the single
sanctioned drop shadow in the product, and it exists because a hairline alone
could not separate a sticky header from content moving beneath it. It is an
exception by design. If you find yourself adding a second one, the answer is
almost always a luminance step or a border instead.

---

## 7. Glass (Prism)

Prism is the layered-glass treatment for **floating elements only**: command
palette, dropdowns, popovers, tooltips, the expanded color picker, toasts. Never
for primary surfaces.

The system ships three classes, and only one is literal glass:

- **`.glass`** is true translucency: `var(--glass-bg)` plus
  `backdrop-filter: blur(12px) saturate(1.15)` and a 1px `--glass-border`.
- **`.glass-dense`** is a *solid* `--surface-overlay`, no blur.
- **`.glass-subtle`** is a *solid* `--surface-raised`, no blur.

The "dense" and "subtle" variants are solid on purpose. Real `backdrop-filter` is
expensive and, stacked, turns text mushy; most panels that once wanted glass read
better, and faster, as a solid raised surface. Reach for `.glass` only when the
translucency genuinely communicates layering. Under `prefers-reduced-motion`,
`.glass` drops its `backdrop-filter` entirely.

---

## 8. Motion

Motion is purposeful, never decorative. Every animation communicates one of
three things: an entrance, a state change, or feedback. Spring easing
(`--ease-spring`) is for interactive elements that should feel physical: buttons,
toggles, thumbs. Silk easing (`--ease-silk`) is for transitions and reveals.

### 8.1 Entrance Animations

Seven entrance keyframes, each with an `.animate-enter-*` utility class. Pair
them with `.stagger-1` through `.stagger-12` (30ms steps) for the one
orchestrated cascade per navigation.

| Class | Keyframe | Duration | Easing |
| ----- | -------- | -------- | ------ |
| `.animate-enter-up` | `enter-up` | 350ms | silk |
| `.animate-enter-fade` | `enter-fade` | 250ms | silk |
| `.animate-enter-right` | `enter-right` | 350ms | silk |
| `.animate-enter-left` | `enter-left` | 350ms | silk |
| `.animate-enter-scale` | `enter-scale` | 250ms | spring |
| `.animate-enter-pop` | `enter-pop` | 350ms | spring |
| `.animate-enter-down` | `enter-down` | 300ms | silk |

The `enter-*` prefix is the convention; new entrance animations join it.

### 8.2 Micro-Interactions

Reusable interaction classes, all tuned to feel like physical light:

- `.card-hover`: **no lift.** The border brightens, an ambient glow emanates, and
  `brightness(1.06)` warms the surface; pressing squishes to `scale(0.97)`. Cards
  used to translate upward; they no longer do.
- `.btn-press`: brightens on hover, presses down to `scale(0.92)`.
- `.chip-interactive`: scales up with a glow halo.
- `.nav-item-hover`: inner glow, no translation.
- `.player-btn`, `.toggle-track` with `.toggle-thumb`, and `.toolbar-action`:
  scale plus glow.

### 8.3 Continuous and Ambient

`breathe`, `borderGlow`, `edgeShimmer`, `dotPulse`, `shimmer`, `glowPulse`,
`eqBar`, and others drive living detail: pulsing status dots, breathing active
states, equalizer bars. Use them sparingly; they are seasoning.

### 8.4 Effect-Swap Cinematics

When the active effect changes, the preview cabinet runs a deliberate ignition
sequence. `canvasIgnite` blooms the canvas in from a dim curtain tinted with the
new accent, `cabinetIgnite` pulses an accent glow behind the cabinet border, and
`effectSwap` (with staggered `-2` and `-3` variants) settles the new title,
description, and category chip into focus. This is rationed drama (§1, principle
4): the payoff moment for the product's core action.

### 8.5 Reduced Motion

`@media (prefers-reduced-motion: reduce)` suppresses entrance, continuous,
ignition, and logo animations, and disables `backdrop-filter` and the noise
overlay. **Any new continuous or entrance animation must be added to that
block.** Motion is an enhancement, never a requirement.

---

## 9. Ambient Reactivity

The signature behavior. The UI subtly takes on the color of the light it is
controlling.

**How it works.** A component samples the live canvas frame for a dominant hue.
The math is a circular mean over pixel data, using sin and cos so the wrap at
360° is handled correctly. The result is written to `--ambient-hue` on the shell
root, and every `--ambient-*` token derives from that hue in OKLCH.

| Token | Intensity | Tints |
| ----- | --------- | ----- |
| `--ambient-glow` | 6 to 8% | Shell edge gradient, preview halo |
| `--ambient-border` | 12 to 15% | Scrollbar thumb, active card border |
| `--ambient-tint` | 3 to 4% | "Now Playing" background, noise tint |

`.preview-glow` crossfades over 2s on the `--ease-silk` curve, so hue shifts
drift rather than snap.

**Discipline.** Hue extraction runs on a low-priority cadence, not the render
loop and not every frame. The CSS property update is one cheap `setProperty`
call; OKLCH interpolation happens in CSS. When no effect is active,
`--ambient-hue` falls back to `320` (purple), matching the primary accent.
Ambient reactivity is a progressive enhancement: the UI is correct and complete
without it.

---

## 10. Noise and Texture

A fractal-noise SVG overlay (`.noise-overlay::before`, fixed, `z-index: 9999`)
gives flat surfaces tactile materiality. Intensity is a token, `--noise-opacity`,
set to **1.4% in dark mode and 0.6% in light**, kept deliberately low so it
grains the surface without ever reading as dirt. The overlay is removed entirely
under `prefers-reduced-motion`.

---

## 11. The Hypercolor Logo

One canonical trinity mark, quarantined to the sidebar. The glyph itself never
moves. The light behind it does, and that single slow drift is the whole of the
brand's motion budget.

The expanded rail renders the vertical lockup
(`/assets/brand/lockup-vertical-color.png` at `h-24`) over a `.logo-bg-mark`
aura: a radial gradient of the accent purple at 18% opacity fading to
transparent at 65%, blurred 6px, running `markChillAura` for a full 360 degree
hue rotation over 24 seconds, linear and infinite. The collapsed rail swaps to
the square mark (`/assets/brand/mark-color.png` at `w-8 h-8`) with no aura
behind it. Both images carry
`.logo-mark-image`, a 14px accent drop shadow at 32% opacity.

`.logo-container` presses to `scale(0.97)` on `:active` and still sets
`cursor: pointer`, which is a leftover affordance: nothing binds a click
handler to the mark today. `.logo-bg-mark` sits in the reduced-motion block, so
the aura holds a still frame when the user asks for less motion.

The mark's restraint is *principle 1 applied to the brand itself*. The one
place the system could justify spending drama, it spends a hue drift instead.
Do not reintroduce per-mode variants, and do not let brand motion migrate into
components.

---

## 12. Component Patterns

### 12.1 Cards

Panel and settings cards sit on `--surface-overlay` with a 1px
`--border-subtle` edge and `rounded-xl`. The effect card keeps that border and
radius but supplies its own background: a radial gradient built from the
category triplet, with a cached thumbnail and then a curated cover image
painted over it when either is available.

Hover is `.card-hover`: the border brightens to `--accent-muted`, a
`--glow-rgb` bloom emanates, and the whole card goes to `brightness(1.06)`.
There is no lift and no translation. Press squishes to `scale(0.97)`.

The active effect card does not animate. It holds a brighter `--accent` border
at 50%, an inset accent ring, and a short `--accent` bar centered on the top
edge. Continuous `breathe` belongs to device cards and dashboard charts, not to
effect cards.

### 12.2 Sidebar

Surface `--surface-raised`. Brand mark on top, nav items (icon plus label), a
spacer, the ambient-tinted "Now Playing" block with transport controls, and a
collapse toggle. Active nav item: a 3px left bar in `--accent` carrying its own
accent glow, and the label lifts to `--text-primary`. There is no background
wash on the active item. Inactive items sit at `--text-tertiary` and take a
`--surface-hover` wash plus a 24px inner glow on hover, with no translation.

### 12.3 Controls

- **Slider.** The base `input[type="range"]` is a 5px `--border-subtle` track
  with a 14px `--accent` thumb and a `--glow-accent` bloom that brightens and
  scales 1.15x on hover. `.slider-silk` is the refined variant: a 4px
  translucent white track with a 14px white thumb whose glow reads
  `--glow-rgb`, scaling 1.2x on hover and 0.95x on press. `.color-channel`
  overrides the base with a 10px transparent track and a white thumb.
- **Toggle.** The track glows when on (`.toggle-track-on`); the thumb springs
  across with a halo.
- **Color picker.** A glowing swatch button expands into a floating
  `.color-picker-popover` (hex input, preview, quick-pick grid, RGB channels).
- **Select.** `SilkSelect` (`src/components/silk_select.rs`) is the canonical
  dropdown: a trigger button plus a *portaled* option panel, with click-outside
  dismissal, scroll-close, Escape, arrow/Home/End keyboard navigation, and
  `listbox`/`option` ARIA. It exists because a native `<option>` popup cannot
  be themed on Linux and Firefox, and because cards are `overflow-hidden`, so
  an in-card dropdown must portal or it gets clipped. `.select-silk` styles a
  bare native `<select>` (custom chevron, `--surface-sunken` background, accent
  focus) and is only for the few places a native control is genuinely wanted.

### 12.4 Focus

Focus is a **glow ring, never a browser outline.** `:focus-visible` applies a 2px
ring plus a 12px soft bloom in `--glow-focus` (cyan in dark, purple in light).
`.glow-ring` pulses once on focus, then holds. This is universal: every focusable
element gets it for free from the base reset.

---

## 13. Accessibility

- **Contrast.** Every text-on-surface pairing meets WCAG 2.1 AA (4.5:1 body,
  3:1 large). OKLCH's perceptual uniformity makes this guarantee tractable.
- **Focus.** The glow ring (§12.4) is visible on every surface level, in both
  themes.
- **Reduced motion.** `prefers-reduced-motion` suppresses animation, ambient
  transitions, glass blur, and the noise overlay (§8.5).
- **Color is never the sole signal.** Category badges carry text labels, not just
  a color dot; status pairs an icon with its color.
- **Both themes ship.** Light mode is held to the same contrast and focus bar as
  dark.

---

## 14. Working in Luminary

Practical rules. Agents and contributors doing UI work should treat this section
as the checklist.

1. **Reference semantic tokens, never raw values.** No `oklch(...)` or `#hex` in
   component CSS or Leptos `style` attributes; use a Tier 2 token or a Tier 3
   Tailwind alias. Raw color in a component is drift.
2. **Stay on the scales.** Corners are `--radius-sm/md/lg/xl` (2/4/6/8px), pills
   `999px`, circles `50%`, nothing between. Spacing is the `--spacing-*` ramp
   (4/8/16/24/32px).
3. **Depth is luminance and borders.** No new `box-shadow` for elevation. The
   page header (§6.3) is the only exception. Glows are color, not depth, and are
   fine.
4. **Colored glows go through `--glow-rgb`.** Set it with an `.accent-*` class,
   or inline from a `category_style()` triplet for dynamic color. Never spray raw
   `rgba()` triplets.
5. **Glass is for floating elements only.** Primary surfaces are solid. Default
   to `.glass-dense` or `.glass-subtle` (both solid); use real `.glass` only when
   translucency earns its cost.
6. **Both themes, every time.** Never assume dark. Verify `[data-theme="light"]`
   before a surface is done.
7. **Motion is rationed and optional.** One orchestrated entrance per view. Add
   every new continuous or entrance animation to the `prefers-reduced-motion`
   block (§8.5).
8. **Focus is a glow ring.** Never reintroduce `outline`.
9. **New tokens live in `tokens/`.** Add to `primitives.css` (Tier 1) and map
   into both theme blocks of `semantic.css` (Tier 2). Not `tailwind.config.js`;
   Tailwind v4 ignores it.
10. **Verify visually.** Surfaces are checked with `agent-browser` against this
    guide: token use, motion, elevation, empty states, component reuse.

When this guide and the shipped CSS disagree, the CSS is authoritative, and
fixing this guide is part of the task.

> **Where rules 1 and 4 stand today.** They are the target, not a description
> of the current tree. `crates/hypercolor-ui/src/` holds 414 raw `rgba()`
> triplets, 317 of them a color other than pure black or white, plus 72
> six-digit hex literals outside `vendors.rs`. The accent purple triplet
> `225, 53, 255` is hardcoded in 31 files under `src/`. Treat the two rules as
> the standard new code is held to; the size and priority of the migration
> backlog is an open call for the design system's owner.

---

## 15. What Luminary Is and Is Not

- **It is the visual language and token system.** It is not a component library;
  component implementations live in `crates/hypercolor-ui/src/`.
- **It is dark-primary.** Light mode is a first-class complement, not a separate
  design and not the default.
- **It is the only visual vocabulary for Hypercolor surfaces.** New surfaces ship
  at the Luminary bar on the wave they land; there is no "make it pretty later"
  pass.
- **It governs application chrome, not effects.** LED effect color science is a
  different discipline with its own reference
  ([`docs/color-science-led-guide.md`](color-science-led-guide.md) and the
  `rgb-effect-design` skill). Do not apply Luminary tokens to effect output, or
  effect-gamut reasoning to UI chrome.

---

## Appendix A: Research Lineage

Luminary's decisions were informed by competitive analysis of:

- **Razer Synapse 3/4:** single-accent discipline, dark-only rationale, the
  Chroma Studio layer metaphor.
- **Linear:** LCH color space, three-variable theme generation, elevation through
  opacity.
- **Vercel Geist:** three-tier token architecture, semantic naming.
- **Arc Browser:** the scrim concept (UI as a frame for content), ambient color.
- **Raycast:** a bold singular accent, noise texture, keyboard-first focus.
- **Material Design 3:** the dark-theme elevation model (brighter is higher).
- **Radix Themes:** stepped color scales, class-based theme switching.

The SilkCircuit palette is Bliss's cross-project visual identity; Luminary is its
application to a real-time control surface.

## Appendix B: Token Naming Convention

Semantic token names follow `--{category}-{property}[-{variant}]`:

```
--surface-base      category=surface,  property=base
--text-primary      category=text,     property=primary
--border-subtle     category=border,   property=subtle
--accent-muted      category=accent,   property=muted
--ambient-glow      category=ambient,  property=glow
--glass-bg          category=glass,    property=bg
--status-success    category=status,   property=success
```

This keeps tokens grep-able, autocomplete-friendly, and self-documenting. Tier 3
Tailwind aliases intentionally shorten `text` and `border` to `fg` and `edge` to
avoid the `text-text-*` doubling (see §3.4).

---

*This guide supersedes `docs/design/19-luminary-design-system.md`, which now
redirects here. Keep it reconciled with `crates/hypercolor-ui/tokens/` and
`input.css`. A drifted style guide is worse than none.*
