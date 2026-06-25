+++
title = "Accent discipline"
description = "The purple-only chrome rule, how to tell chrome from semantic color, and the de-cyan checklist for the docs theme."
weight = 20
+++

# Accent discipline 💜

Electric purple is the only accent color in Luminary UI chrome. Every button, toggle, active-state indicator, slider thumb, sidebar active bar, and focus partner is purple. That single confident accent is what separates a control surface from a toy.

Every other SilkCircuit color — cyan, coral, yellow, green, red, blue — enters the interface through semantic channels only: status signals, category identity, or data encodings. If you reach for cyan to style a link hover or a nav item, stop. That is a category or status signal leaking into chrome.

This page explains the rule, shows exactly which contexts are chrome versus semantic, and provides a concrete checklist for auditing drift.

## The rule

`var(--accent)` resolves to `oklch(0.65 0.30 320)` in dark mode and `oklch(0.58 0.28 320)` in light. Hover, muted, and subtle variants derive from the same hue:

| Token | Use |
| ----- | --- |
| `var(--accent)` | Active states, labels, borders |
| `var(--accent-hover)` | Hover brightening |
| `var(--accent-muted)` | 12% wash — selected surface tint |
| `var(--accent-subtle)` | 6% wash — hover surface tint |
| `var(--glow-accent-50)` | Text-shadow / box-shadow glow at 50% alpha |

Never reach for a raw `oklch()` or hex value in a component rule; always route through a token. Hardcoded color in a component defeats theme switching.

## Semantic-vs-chrome decision table

| Color | Allowed in chrome? | Correct semantic use |
| ----- | ------------------ | -------------------- |
| Purple (`--color-purple`) | Yes — the only chrome color | Buttons, toggles, active bars, focus ring in light mode |
| Cyan (`--color-cyan`) | One exception only | Dark-mode focus glow ring (`--glow-focus`) |
| Coral (`--color-coral`) | No | Inline code text (sanctioned "code is special" semantic); audio and display category badges |
| Yellow (`--color-yellow`) | No | `--status-warning`; reactive and source category badges |
| Green (`--color-green`) | No | `--status-success`; generative category badges |
| Red (`--color-red`) | No | `--status-error`; danger callouts |
| Blue (`--color-blue`) | No | `--status-info`; interactive category badges; API method badges |
| Soft pink (`255, 153, 255`) | No | Productivity category badges |

The one cyan exception is the focus glow ring in dark mode. `DESIGN-SYSTEM.md` §12.4 specifies cyan focus in dark and purple focus in light, so `:focus-visible` reads `--glow-focus`, which resolves to cyan in dark and flips to purple in light. This is the single correct use of cyan on a structural element — everywhere else, cyan is a category or status signal.

## Category color map

Effect categories carry their own color identity. This flows through badges and `--glow-rgb` effects, not through chrome. The canonical source of truth is `category_style()` in `crates/hypercolor-ui/src/style_utils.rs`. Always copy triplets from that file, never from memory or docs.

| Category | Color | RGB triplet |
| -------- | ----- | ----------- |
| `ambient` | Neon cyan | `128, 255, 234` |
| `audio` | Coral | `255, 106, 193` |
| `display` | Coral | `255, 106, 193` |
| `gaming` | Electric purple | `225, 53, 255` |
| `reactive` | Electric yellow | `241, 250, 140` |
| `source` | Electric yellow | `241, 250, 140` |
| `generative` | Success green | `80, 250, 123` |
| `interactive` | Info blue | `130, 170, 255` |
| `productivity` | Soft pink | `255, 153, 255` |
| `utility`, unknown | Tertiary gray | `139, 133, 160` |

Category color is **identity**, not chrome. A category badge paints the badge; it does not leak into navigation, headings, or interactive controls. A gaming effect's purple badge is not the same job as the sidebar active indicator's purple — the badge is data, the sidebar bar is chrome.

When a component needs to glow in a category's color, set `--glow-rgb` to the triplet above and let `.edge-glow-accent` or `.chip-interactive` read it. Never hardcode a raw `rgba(...)` triplet in a component.

## The `--glow-rgb` system

CSS cannot extract RGB channels from an `oklch()` value, but colored glows and `rgba()` gradients need numeric channels. The solution is a single custom property, `--glow-rgb`, holding an `R, G, B` triplet that glow utilities read via `rgba(var(--glow-rgb), alpha)`.

Named accent classes set it for static use:

```css
.accent-purple  { --glow-rgb: 225, 53, 255; }   /* brand / chrome */
.accent-cyan    { --glow-rgb: 128, 255, 234; }   /* live state only */
.accent-coral   { --glow-rgb: 255, 106, 193; }   /* display-face chrome */
.accent-yellow  { --glow-rgb: 241, 250, 140; }   /* warnings */
.accent-green   { --glow-rgb: 80, 250, 123;  }   /* success */
.accent-red     { --glow-rgb: 255, 99, 99;   }   /* errors */
```

For dynamic category color — a card glowing in its effect's category — set `--glow-rgb` inline from the `category_style()` triplet:

```rust
// in a Leptos component
let (_, rgb) = category_style(effect.category.as_str());
view! {
    <div
        class="card-hover edge-glow-accent"
        style=format!("--glow-rgb: {rgb}")
    >
        // ...
    </div>
}
```

Device cards use `device_accent_colors()` from the same file, which runs an FNV-1a hash over the device ID to produce a deterministic two-color gradient. Same device, same colors, every session — identity without a stored palette.

## De-cyan checklist

Use this when reviewing any new component or auditing existing code for accent discipline violations. Chrome leaks are the most common class of Luminary drift.

### Structural elements (must be purple)

- [ ] Nav links: hover is `var(--accent)` foreground with `var(--accent-subtle)` background and `var(--glow-accent-50)` text-shadow. No cyan.
- [ ] Nav link active indicator: the 2px `::after` bar is `var(--accent)`.
- [ ] Nav title hover: the `.nav-title__color` half shifts to `var(--accent)`. The `.nav-title__hyper` gradient sweep is the sanctioned brand exception.
- [ ] Sidebar group titles: `color: var(--accent)`. The `›` caret uses `var(--text-tertiary)` — not cyan.
- [ ] Sidebar link hover: `background: var(--accent-subtle)`. No cyan wash.
- [ ] Sidebar active link: 3px left border in `var(--accent)`, `var(--accent-subtle)` background, glow via `var(--glow-accent-50)`.
- [ ] TOC active and hover links: `var(--accent)` / `var(--accent-subtle)`.
- [ ] Content links: `color: var(--accent)`, underline-grow in `var(--accent)`, hover glow via `var(--glow-accent-50)`.
- [ ] H1 gradient: purple-to-purple (`var(--accent)` to `var(--accent-hover)`). No cyan stop.
- [ ] H2 `◆` prefix: `color: var(--accent)`.
- [ ] Table header text and bottom border: `color: var(--accent)` and `border-bottom: 2px solid var(--accent)`.
- [ ] Blockquote left bar: `border-left-color: var(--accent)`.
- [ ] Theme toggle hover: `color: var(--accent)`, `border-color: var(--accent-muted)`, glow via `var(--glow-accent-50)`.
- [ ] Search trigger hover: `background: var(--accent-subtle)`. No cyan.
- [ ] Feature card titles and icons on the landing: `var(--accent)`. Not cyan.
- [ ] Footer links: `var(--accent)` resting. Not cyan.
- [ ] `btn--secondary` CTAs: purple outline (`var(--accent)` border and text).

### Semantic elements (may keep non-purple color)

- [ ] Focus ring: `--glow-focus` (cyan in dark, purple in light). This is the one correct cyan use on a structural element.
- [ ] Callouts (`tip`, `warning`, `danger`, `info`): route through `--status-*` tokens, not hardcoded OKLCH values, so they flip correctly in light mode.
- [ ] API method badges: `GET` cyan, `POST` green, `PUT`/`PATCH` yellow, `DELETE` red. Data encodings, not chrome — keep their colors.
- [ ] Inline code text: `var(--color-coral)` — sanctioned "code is special" semantic, routed through the palette token so it swaps with the theme.
- [ ] Category badges: all colors from the category map above. Identity, not chrome.
- [ ] `--glow-rgb` inline from `category_style()` on effect or device cards: correct. Category color in data context, not in nav or controls.

### Token hygiene

- [ ] No raw `oklch(0.88 0.18 175 / ...)` in component rules — that is hardcoded cyan that does not theme-swap. Use `var(--color-cyan)` or `--glow-focus` as appropriate.
- [ ] No raw `oklch(0.65 0.30 320 / ...)` in component rules. Use `var(--glow-accent-50)`, `var(--glow-accent-70)`, or `var(--glow-accent-80)`.
- [ ] No hardcoded `rgba(225, 53, 255, ...)` triplets in component CSS. Set `--glow-rgb: 225, 53, 255` and let `.edge-glow-accent` read it.
- [ ] Every token that appears in the dark theme block must appear in the light block too. A missing light-mode token leaks the dark value onto bright surfaces.

## Common violation patterns

These are the leak patterns that recur most often.

**Nav hover going cyan.** A previous version of the docs nav used cyan for hover background and text-shadow. The current `_nav.scss` shows the corrected state: hover is `var(--accent-subtle)` background with `var(--glow-accent-50)` text-shadow. If you see cyan there again, it has re-drifted.

**Sidebar group caret.** The `›` glyph before group titles was previously styled cyan. The caret's job is structural punctuation. It belongs in `var(--text-tertiary)`, confirmed in the current `_sidebar.scss`.

**H1 gradient with a cyan stop.** A purple-to-cyan-to-purple gradient reads as decorative rather than branded. H1 uses a purple-only gradient (`var(--accent)` to `var(--accent-hover)`), confirmed in `_content.scss`. The `.nav-title__hyper` gradient sweep on the wordmark retains the cyan mid-stop as the explicitly sanctioned brand exception.

**Callouts hardcoding dark-mode OKLCH.** Callout border and icon colors written as literal `oklch(0.88 0.18 175 / ...)` do not swap in light mode and will fail contrast on bright surfaces. Route through `--status-*` tokens.

**Feature card titles and icons.** Cards are chrome. Their title text and icon glow must be purple, not cyan, even when the card describes an ambient (cyan) effect category. The category badge on the card carries the category color; the card chrome stays purple.

## Related pages

- [@/theme/tokens.md](@/theme/tokens.md) — full Tier 1 / Tier 2 token tables and how the three-tier architecture works
- [@/theme/contributing.md](@/theme/contributing.md) — how to add a styled component without drifting from Luminary
- [`docs/DESIGN-SYSTEM.md`](https://github.com/hyperb1iss/hypercolor/blob/main/docs/DESIGN-SYSTEM.md) — the canonical reference; when the code and this page disagree, the code wins
