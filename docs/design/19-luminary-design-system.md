# Luminary Design System

Hypercolor's visual language — a cinematic control surface that lives and breathes light.

**Codename:** Luminary (ambient reactivity) + Prism (layered glass)
**Status:** Spec — not yet implemented
**Supersedes:** Ad-hoc SilkCircuit token usage in `input.css`

---

## 1. Philosophy

The UI is a **dark scrim around light**. Inspired by Robert Irwin's theatrical scrims and VJ
lighting consoles, the interface defers to the RGB effects it controls. Dark, restrained chrome
provides the stage. The effects — previewed live on canvas — provide the drama.

Three principles:

1. **The light is the star.** UI chrome is neutral and recessive. Color enters through effect
   previews, category semantics, and ambient reactivity — not decorative UI elements.
2. **Depth through luminance, not shadow.** Higher elevation = brighter surface (white opacity
   overlays). Shadows don't work on dark backgrounds — there's nowhere darker to go.
3. **The UI breathes.** A single `--ambient` CSS variable, derived from the active effect's
   dominant hue, tints borders, scrollbars, and edge glows. The interface subtly mirrors the
   light it orchestrates.

---

## 2. Token Architecture

Three-tier system inspired by Linear and Geist. Mode switching happens at Tier 2 only.

```
Tier 1 (Primitive)   →  Raw color values in OKLCH
Tier 2 (Semantic)    →  Intent-mapped tokens, swapped per theme
Tier 3 (Component)   →  Usage-specific, theme-agnostic
```

### 2.1 Switching Mechanism

Theme is controlled by a `data-theme` attribute on `<html>`:

```html
<html data-theme="dark">   <!-- default -->
<html data-theme="light">
```

Leptos toggles this attribute. CSS uses attribute selectors:

```css
:root, [data-theme="dark"]  { /* dark semantic tokens */ }
[data-theme="light"]         { /* light semantic tokens */ }
```

User preference is persisted to `localStorage` and respects `prefers-color-scheme` on first visit.

### 2.2 Tier 1 — Primitives

Raw values. Never referenced directly in components.

```css
@theme {
    /* ── Neutrals (OKLCH, hue 280 = violet undertone) ── */
    --color-void-1:     oklch(0.110 0.020 280);   /* deepest */
    --color-void-2:     oklch(0.130 0.018 280);
    --color-void-3:     oklch(0.155 0.016 280);
    --color-void-4:     oklch(0.185 0.014 280);
    --color-void-5:     oklch(0.220 0.012 280);
    --color-void-6:     oklch(0.280 0.010 280);

    --color-cloud-1:    oklch(0.985 0.005 280);   /* brightest */
    --color-cloud-2:    oklch(0.960 0.008 280);
    --color-cloud-3:    oklch(0.930 0.010 280);
    --color-cloud-4:    oklch(0.890 0.012 280);
    --color-cloud-5:    oklch(0.840 0.014 280);
    --color-cloud-6:    oklch(0.780 0.016 280);

    /* ── SilkCircuit Accent Palette ── */
    --color-purple:     oklch(0.65 0.30 320);      /* #e135ff — primary accent */
    --color-cyan:       oklch(0.88 0.18 175);      /* #80ffea — interactive focus */
    --color-coral:      oklch(0.72 0.22 350);      /* #ff6ac1 — secondary accent */
    --color-yellow:     oklch(0.93 0.15 105);      /* #f1fa8c — warnings, attention */
    --color-green:      oklch(0.85 0.22 155);      /* #50fa7b — success */
    --color-red:        oklch(0.68 0.22 25);       /* #ff6363 — error, danger */
    --color-blue:       oklch(0.72 0.12 260);      /* #82aaff — info */

    /* ── Typography ── */
    --font-sans:  'Satoshi', 'Inter', system-ui, sans-serif;
    --font-mono:  'JetBrains Mono', 'Fira Code', ui-monospace, monospace;
    --font-display: 'Satoshi', system-ui, sans-serif;

    /* ── Motion ── */
    --ease-silk:    cubic-bezier(0.4, 0, 0.2, 1);
    --ease-spring:  cubic-bezier(0.34, 1.56, 0.64, 1);
    --ease-out:     cubic-bezier(0, 0, 0.2, 1);
    --duration-fast:   120ms;
    --duration-normal: 200ms;
    --duration-slow:   400ms;

    /* ── Radii ── */
    --radius-sm:  6px;
    --radius-md:  10px;
    --radius-lg:  14px;
    --radius-xl:  20px;
}
```

### 2.3 Tier 2 — Semantic Tokens (Dark Mode)

Intent-mapped. These are what components reference.

```css
:root, [data-theme="dark"] {
    color-scheme: dark;

    /* ── Surfaces (elevation via white overlay) ── */
    --surface-base:      var(--color-void-1);           /* page bg */
    --surface-raised:    var(--color-void-2);           /* sidebar, header */
    --surface-overlay:   var(--color-void-3);           /* cards, panels */
    --surface-sunken:    var(--color-void-4);           /* inputs, wells */
    --surface-hover:     var(--color-void-5);           /* hover states */
    --surface-active:    var(--color-void-6);           /* pressed/selected */

    /* ── Glass (Prism overlays) ── */
    --glass-bg:          oklch(0.13 0.02 280 / 0.70);
    --glass-bg-dense:    oklch(0.13 0.02 280 / 0.85);
    --glass-blur:        16px;
    --glass-saturate:    1.3;
    --glass-border:      oklch(1 0 0 / 0.06);

    /* ── Text ── */
    --text-primary:      oklch(0.96 0.01 280);          /* near-white, not pure */
    --text-secondary:    oklch(0.68 0.03 280);          /* muted labels */
    --text-tertiary:     oklch(0.52 0.04 280);          /* placeholders, disabled */
    --text-inverse:      var(--color-void-1);

    /* ── Borders ── */
    --border-subtle:     oklch(1 0 0 / 0.06);
    --border-default:    oklch(1 0 0 / 0.10);
    --border-strong:     oklch(1 0 0 / 0.16);
    --border-focus:      oklch(0.88 0.18 175 / 0.40);   /* cyan focus ring */

    /* ── Accent ── */
    --accent:            var(--color-purple);
    --accent-hover:      oklch(0.70 0.30 320);
    --accent-muted:      oklch(0.65 0.30 320 / 0.12);
    --accent-subtle:     oklch(0.65 0.30 320 / 0.06);

    /* ── Ambient (dynamic — set from JS) ── */
    --ambient-hue:       320;        /* default: purple */
    --ambient-glow:      oklch(0.65 0.20 var(--ambient-hue) / 0.08);
    --ambient-border:    oklch(0.65 0.20 var(--ambient-hue) / 0.15);
    --ambient-tint:      oklch(0.65 0.20 var(--ambient-hue) / 0.04);

    /* ── Semantic status ── */
    --status-success:    var(--color-green);
    --status-error:      var(--color-red);
    --status-warning:    var(--color-yellow);
    --status-info:       var(--color-blue);

    /* ── Scrollbar ── */
    --scrollbar-thumb:   oklch(0.65 0.30 320 / 0.15);
    --scrollbar-hover:   oklch(0.65 0.30 320 / 0.30);
}
```

### 2.4 Tier 2 — Semantic Tokens (Light Mode)

```css
[data-theme="light"] {
    color-scheme: light;

    /* ── Surfaces ── */
    --surface-base:      var(--color-cloud-1);
    --surface-raised:    var(--color-cloud-2);
    --surface-overlay:   white;
    --surface-sunken:    var(--color-cloud-3);
    --surface-hover:     var(--color-cloud-4);
    --surface-active:    var(--color-cloud-5);

    /* ── Glass ── */
    --glass-bg:          oklch(0.98 0.005 280 / 0.70);
    --glass-bg-dense:    oklch(0.98 0.005 280 / 0.85);
    --glass-border:      oklch(0 0 0 / 0.08);

    /* ── Text ── */
    --text-primary:      oklch(0.18 0.02 280);
    --text-secondary:    oklch(0.42 0.03 280);
    --text-tertiary:     oklch(0.58 0.02 280);
    --text-inverse:      var(--color-cloud-1);

    /* ── Borders ── */
    --border-subtle:     oklch(0 0 0 / 0.06);
    --border-default:    oklch(0 0 0 / 0.10);
    --border-strong:     oklch(0 0 0 / 0.18);
    --border-focus:      oklch(0.65 0.30 320 / 0.50);   /* purple focus in light */

    /* ── Accent (slightly desaturated for light bg) ── */
    --accent:            oklch(0.58 0.28 320);
    --accent-hover:      oklch(0.52 0.28 320);
    --accent-muted:      oklch(0.58 0.28 320 / 0.10);
    --accent-subtle:     oklch(0.58 0.28 320 / 0.05);

    /* ── Ambient (same mechanism, lower intensity) ── */
    --ambient-glow:      oklch(0.58 0.15 var(--ambient-hue) / 0.06);
    --ambient-border:    oklch(0.58 0.15 var(--ambient-hue) / 0.12);
    --ambient-tint:      oklch(0.58 0.15 var(--ambient-hue) / 0.03);

    /* ── Scrollbar ── */
    --scrollbar-thumb:   oklch(0 0 0 / 0.12);
    --scrollbar-hover:   oklch(0 0 0 / 0.24);
}
```

---

## 3. Typography

Replace Inter with **Satoshi** — a modern geometric sans with more personality than Inter, tighter
default metrics, and excellent weight range. It reads as "designed" rather than "default."

### 3.1 Type Scale

| Level     | Size   | Weight | Tracking   | Use                          |
|-----------|--------|--------|------------|------------------------------|
| Display   | 28px   | 700    | -0.03em    | Page titles                  |
| Title     | 20px   | 600    | -0.02em    | Section headers, card names  |
| Heading   | 16px   | 600    | -0.015em   | Subsections, panel headers   |
| Body      | 14px   | 400    | -0.01em    | Primary content              |
| Label     | 12px   | 500    | 0em        | Control labels, nav items    |
| Caption   | 11px   | 500    | 0.01em     | Metadata, badges, timestamps |
| Micro     | 10px   | 500    | 0.02em     | Tiny labels, status text     |

### 3.2 Mono Scale

JetBrains Mono for all data, metrics, hex values, and code.

| Level     | Size   | Weight | Use                          |
|-----------|--------|--------|------------------------------|
| Mono-lg   | 14px   | 500    | Hex values in pickers        |
| Mono-md   | 12px   | 400    | Status values, FPS counter   |
| Mono-sm   | 10px   | 400    | Slider values, RGB channels  |

### 3.3 Dark Mode Adjustments

In dark mode, text weights are optically lighter due to irradiation. Compensate:
- Body weight in dark: 400 (normal)
- Body weight in light: 350 (if available) or keep 400 with `font-optical-sizing: auto`
- Display/Title: +50 weight in dark mode (`font-weight: 700` → visually matches light `650`)

---

## 4. Surfaces & Elevation

### 4.1 Layer Model

```
┌──────────────────────────────────────────────────┐
│ surface-base          (page background)           │
│  ┌────────────────────────────────────────────┐  │
│  │ surface-raised     (sidebar, header)        │  │
│  │  ┌──────────────────────────────────────┐  │  │
│  │  │ surface-overlay  (cards, panels)      │  │  │
│  │  │  ┌────────────────────────────────┐  │  │  │
│  │  │  │ surface-sunken (inputs, wells)  │  │  │  │
│  │  │  └────────────────────────────────┘  │  │  │
│  │  └──────────────────────────────────────┘  │  │
│  └────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────┘
```

**Dark mode:** Each layer is slightly brighter (white overlay at increasing opacity).
**Light mode:** Each layer is slightly darker (black overlay) or uses distinct cloud tones.
**No box-shadows** for elevation. Borders only: `--border-subtle` (1px) between layers.

### 4.2 Glass Panels (Prism)

Used **only** for floating elements that benefit from depth:
- Command palette
- Dropdown menus
- Popovers / tooltips
- Color picker expanded panel
- Toast notifications

**Not** for primary surfaces (sidebar, cards, content areas).

```css
.glass {
    background: var(--glass-bg);
    backdrop-filter: blur(var(--glass-blur)) saturate(var(--glass-saturate));
    border: 1px solid var(--glass-border);
}

.glass-dense {
    background: var(--glass-bg-dense);
    backdrop-filter: blur(20px) saturate(var(--glass-saturate));
    border: 1px solid var(--glass-border);
}
```

---

## 5. Ambient Reactivity System

The killer differentiator. The UI subtly reflects the active effect's color.

### 5.1 How It Works

1. The effect engine extracts a **dominant hue** from the active effect's canvas output
   (simple hue averaging, updated every ~500ms — NOT every frame).
2. Leptos sets `--ambient-hue` as a CSS custom property on the shell root.
3. All `--ambient-*` tokens derive from this hue via OKLCH.
4. Affected elements: shell edge glow, scrollbar accent, active card border, sidebar
   "now playing" section background, noise overlay tint.

### 5.2 What Gets Tinted

| Element                    | Token Used          | Intensity |
|----------------------------|---------------------|-----------|
| Shell edge radial gradient | `--ambient-glow`    | Very subtle (4-8%) |
| Scrollbar thumb            | `--ambient-border`  | Medium (15%) |
| Active effect card border  | `--ambient-border`  | Medium (15%) |
| Sidebar "Now Playing" bg   | `--ambient-tint`    | Subtle (4%) |
| Noise overlay tint         | `--ambient-tint`    | Very subtle (3%) |

### 5.3 Fallback

When no effect is active, `--ambient-hue: 320` (purple) — matching the primary accent.
The ambient system is a progressive enhancement; the UI works perfectly without it.

### 5.4 Performance

- Hue extraction runs on a low-priority timer, not the render loop
- CSS custom property updates are cheap — single `style.setProperty()` call
- OKLCH interpolation happens in CSS, not JS
- No `backdrop-filter` on ambient elements — only `background` and `border-color`

---

## 6. Color System

### 6.1 Accent Usage Rules

**Electric Purple** (`--accent`) is the only accent used in UI chrome:
- Interactive elements: buttons, toggles, active states
- Focus indicators (with cyan for keyboard focus)
- The sidebar active indicator bar
- Slider thumbs

**Other SilkCircuit colors** appear ONLY in semantic contexts:
- Category badges (ambient=cyan, audio=coral, gaming=purple, etc.)
- Status indicators (success=green, error=red, warning=yellow)
- Data visualization and device backend indicators

### 6.2 Category Color Map

```rust
fn category_color(cat: &str) -> &str {
    match cat {
        "ambient"      => "var(--color-cyan)",
        "audio"        => "var(--color-coral)",
        "gaming"       => "var(--color-purple)",
        "reactive"     => "var(--color-yellow)",
        "generative"   => "var(--color-green)",
        "interactive"  => "var(--color-blue)",
        "productivity" => "var(--color-coral)",   /* shared with audio */
        "utility"      => "var(--text-tertiary)",
        _              => "var(--text-secondary)",
    }
}
```

Category colors are applied via inline `style` (not Tailwind classes) because they're dynamic.
In light mode, category colors automatically desaturate slightly via the OKLCH primitive shift.

---

## 7. Motion System

### 7.1 Principles

- **Purposeful, not decorative.** Every animation communicates: entrance, state change, or feedback.
- **One orchestrated moment per navigation.** Staggered card entrance on page load. That's the "wow."
  Not scattered micro-interactions on every hover.
- **Spring for interactive, silk for transitions.** Buttons/toggles use `--ease-spring`. Page
  transitions and reveals use `--ease-silk`.

### 7.2 Duration Scale

| Token              | Value  | Use                              |
|--------------------|--------|----------------------------------|
| `--duration-fast`  | 120ms  | Hover, focus, press feedback     |
| `--duration-normal`| 200ms  | Toggles, color changes, state    |
| `--duration-slow`  | 400ms  | Page transitions, panel reveals  |

### 7.3 Entrance Animations (Preserved)

Keep all existing keyframes — they're well-designed. Rename for consistency:

| Current Name       | New Name              | Duration | Easing       |
|--------------------|-----------------------|----------|--------------|
| `fadeInUp`         | `enter-up`            | 350ms    | silk         |
| `fadeIn`           | `enter-fade`          | 250ms    | silk         |
| `slideInRight`     | `enter-right`         | 350ms    | silk         |
| `slideInLeft`      | `enter-left`          | 350ms    | silk         |
| `scaleIn`          | `enter-scale`         | 250ms    | spring       |
| `popIn`            | `enter-pop`           | 350ms    | spring       |
| `slideDown`        | `enter-down`          | 300ms    | silk         |

### 7.4 Micro-Interactions (Preserved)

Keep `card-hover`, `btn-press`, `chip-interactive`, `nav-item-hover`, `player-btn`,
`toggle-track`, `toggle-thumb`. These are solid. No changes needed.

### 7.5 Ambient Animations (Preserved)

Keep `breathe`, `dotPulse`, `borderGlow`, `shimmer`. Parameterize `breathe` via
`--glow-rgb` (already done) and add `--ambient-hue` support.

---

## 8. Component Patterns

### 8.1 Cards

```
┌─────────────────────────────┐  surface-overlay
│  ┌───────────────────────┐  │
│  │    Category gradient   │  │  4px top accent (category color, 15% opacity)
│  └───────────────────────┘  │
│  Title                 tag  │  title: heading size, tag: caption mono
│  Description text...        │  body size, text-secondary
│  ┌──┐ ┌──┐ ┌──┐           │  capability badges
│  └──┘ └──┘ └──┘           │
└─────────────────────────────┘
border: 1px solid --border-subtle
hover: border-color → --border-default, translateY(-2px)
active: border-color → --ambient-border, breathe animation
```

### 8.2 Sidebar

```
surface-raised background
──────────────────
Logo / Brand mark
──────────────────
Nav items (icon + label)
  Active: 3px left bar (--accent), bg --accent-subtle
  Hover: bg surface-hover, translateX(2px)
──────────────────
[spacer]
──────────────────
Now Playing section
  bg: --ambient-tint
  Effect name + category dot
  Transport controls (prev/stop/next/shuffle)
──────────────────
Collapse toggle
```

### 8.3 Detail Sidebar (Effects Page)

```
surface-raised, sticky, 420px
border-left: 1px solid --border-subtle
──────────────────
Effect name (title size)
Category dot + author
──────────────────
Canvas preview (live render)
  border-radius: --radius-lg
  ambient glow edge
──────────────────
Preset toolbar
  Glass panel for dropdown
──────────────────
Control panel
  Group headers (caption, centered rule)
  Slider / Toggle / ColorPicker / Dropdown
──────────────────
```

### 8.4 Controls

**Slider:** Track 3px, thumb 14px circle, accent-colored glow. Value badge in accent-muted bg.

**Toggle:** 40x22 track. On: accent bg + glow. Off: surface-sunken. Spring thumb animation.

**Color Picker:** Swatch button (rounded-xl, gradient fill, glow shadow) → inline accordion
expansion. Glass panel: hex input + preview swatch + quick-pick grid + RGB channel sliders.
Close: click swatch again, click outside, or small X button.

**Dropdown:** Native `<select>` styled with surface-sunken bg, border-subtle. Accent focus ring.

---

## 9. Noise & Texture

### 9.1 Noise Overlay

Keep the fractal noise SVG overlay. Increase from 1.5% to **2.5%** opacity in dark mode,
**1%** in light mode. This adds tactile materiality to flat surfaces.

```css
.noise-overlay::before {
    opacity: var(--noise-opacity, 0.025);
}

[data-theme="light"] {
    --noise-opacity: 0.01;
}
```

### 9.2 Aurora Background (Optional)

For the shell's base layer, an optional aurora gradient that shifts with `--ambient-hue`:

```css
.aurora-bg::after {
    content: '';
    position: fixed;
    inset: 0;
    pointer-events: none;
    z-index: -1;
    background: radial-gradient(
        ellipse 80% 50% at 20% 100%,
        oklch(0.30 0.08 var(--ambient-hue) / 0.06),
        transparent 70%
    );
    transition: background 2s var(--ease-silk);
}
```

Very subtle — just enough to break the flat void. Disabled in `prefers-reduced-motion`.

---

## 10. Tailwind v4 Implementation Strategy

### 10.1 File Structure

```
crates/hypercolor-ui/
  input.css              → imports + @theme primitives + base resets
  tokens/
    primitives.css       → Tier 1 OKLCH values
    dark.css             → Tier 2 dark semantic tokens
    light.css            → Tier 2 light semantic tokens
  components/
    glass.css            → Glass panel utilities
    animations.css       → Keyframes + animation utilities
    interactions.css     → Micro-interaction classes
    ambient.css          → Ambient reactivity styles
```

### 10.2 Migration Path

1. Define primitives in OKLCH (new `primitives.css`)
2. Map semantic tokens for dark mode (existing behavior, new names)
3. Add light mode semantic tokens
4. Update `input.css` to import the new structure
5. Add `data-theme` toggle to Leptos shell
6. Migrate components from `bg-layer-N` → `bg-[var(--surface-*)]` (or create Tailwind aliases)
7. Add ambient hue extraction + CSS property updates
8. Add glass utilities to floating elements
9. Swap Inter → Satoshi in `index.html` Google Fonts link

### 10.3 Tailwind Aliases

Register semantic tokens as Tailwind theme extensions so components use readable classes:

```css
@theme {
    /* These reference the semantic tokens, enabling dark: prefix */
    --color-surface-base:    var(--surface-base);
    --color-surface-raised:  var(--surface-raised);
    --color-surface-overlay: var(--surface-overlay);
    --color-surface-sunken:  var(--surface-sunken);
    --color-text-primary:    var(--text-primary);
    --color-text-secondary:  var(--text-secondary);
    --color-text-tertiary:   var(--text-tertiary);
}
```

Usage in Leptos: `class="bg-surface-base text-text-primary border-border-subtle"`

---

## 11. Accessibility

- **Contrast:** All text-on-surface combinations meet WCAG 2.1 AA (4.5:1 for body, 3:1 for large).
  OKLCH perceptual uniformity makes this easier to guarantee.
- **Focus indicators:** Cyan ring (dark) / purple ring (light), 2px + 8px glow. Visible on all
  surface levels.
- **Reduced motion:** All animations suppressed. Ambient hue changes become instant (no transition).
- **Glass panels:** Minimum 4.5:1 contrast for text over blurred backgrounds. Use `glass-dense`
  (85% opacity) for text-heavy content.
- **Color not sole indicator:** Category badges include text labels, not just color dots. Status
  uses icon + color, never color alone.

---

## 12. What This Is NOT

- **Not a component library.** This defines the visual language and token system. Component
  implementation is a separate task.
- **Not a breaking rewrite.** The migration is incremental — existing classes continue to work
  while new tokens are adopted component-by-component.
- **Not a light-mode-first system.** Dark is the primary, fully-featured mode. Light is a
  complement for daytime use, not a separate design.

---

## Appendix A: Research Sources

Design decisions informed by competitive analysis of:
- **Razer Synapse 3/4** — single-accent discipline, dark-only rationale, Chroma Studio layer metaphor
- **SignalRGB** (competitor) — canvas-as-spatial-map, dark + neon accent pairing, effect control type system
- **Linear** — LCH color space, 3-variable theme generation, elevation via opacity
- **Vercel Geist** — 3-tier token architecture, semantic naming conventions
- **Arc Browser** — scrim concept (UI as frame for content), ambient color spaces
- **Raycast** — bold singular accent, noise textures, keyboard-first focus design
- **Material Design 3** — dark theme elevation model (brighter = higher)
- **Radix Themes** — 12-step color scales, class-based dark/light switching

## Appendix B: Font Evaluation

| Font         | Pros                                     | Cons                          | Verdict      |
|--------------|------------------------------------------|-------------------------------|--------------|
| Inter        | Universal, safe, excellent features      | Generic, overused             | Replace      |
| Satoshi      | Geometric, tight, distinctive, free      | Less proven at small sizes    | **Primary**  |
| General Sans | Similar to Satoshi, softer              | Less personality              | Backup       |
| Geist Sans   | Vercel-associated, technical             | Too "developer tool"          | Skip         |
| Space Grotesk| Distinctive but overused in AI/crypto   | Cliche risk                   | Skip         |
| Plus Jakarta | Warm, rounded, approachable              | Too soft for "control surface"| Skip         |

## Appendix C: Naming Conventions

Semantic token names follow the pattern: `--{category}-{property}[-{variant}]`

```
--surface-base       category=surface, property=base
--text-primary       category=text, property=primary
--border-subtle      category=border, property=subtle
--accent-muted       category=accent, property=muted
--ambient-glow       category=ambient, property=glow
--glass-bg           category=glass, property=bg
--status-success     category=status, property=success
```

This makes tokens grep-able, autocomplete-friendly, and self-documenting.
