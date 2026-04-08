# Hypercolor UX Philosophy & Information Architecture

> The lighting engine that looks as good as your setup — and never makes you feel lost.

---

## 1. Design Philosophy

### The Core Tension

A lighting control app must be two things simultaneously: **visually stunning** (you're literally selling visual experiences) and **immediately navigable** (people don't read manuals for RGB software). Most existing tools chose stunning and sacrificed navigable. We choose both.

### Principles

**1. The Light Is the Hero, Not the Chrome**

Every pixel of UI chrome exists to serve the light output. The interface should feel like looking through a window at your lighting — not like operating a cockpit. Dark, muted surfaces. The only vivid color in the UI should be the preview of what your LEDs are actually doing. When an effect is running, the user's eye should go to the live preview first, controls second, navigation third.

**2. Progressive Disclosure, Not Progressive Confusion**

Show the user exactly what they need for their current intent:

```
Level 0: "I want to change my lighting"    → Effect browser, one click
Level 1: "I want to tweak this effect"      → Control panel slides in
Level 2: "I want to map my devices"         → Spatial editor opens
Level 3: "I want to write custom effects"   → Dev tools, API docs
```

Each level is reachable from the previous one. No level requires understanding any deeper level. A gamer who just wants rainbow wave never sees the spatial editor unless they go looking for it.

**3. Spatial Consistency**

Things live where you expect them. Every time. The effect browser is always on the left. The control panel is always on the right. The live preview is always in the center. Navigation doesn't rearrange itself based on context. This is the number one thing proprietary alternatives get wrong — the UI shifts and reorganizes depending on what you clicked, destroying spatial memory.

**4. Keyboard-First, Mouse-Friendly**

Power users drive with the keyboard. Every action has a shortcut. The command palette (`Ctrl+K`) can reach anything in the app from anywhere. But nothing requires the keyboard — every interaction works with mouse/touch. Two input modalities, one coherent experience.

**5. Show, Don't Describe**

No effect should ever be represented by just a name and a description. Every effect has a live animated thumbnail. Every device shows its actual LED colors in real-time. Every control change is reflected instantly in the preview. The user never has to imagine what something will look like.

**6. Error States Are First-Class UI**

A disconnected device isn't a popup. It's a subtle status change on the device's card — a dimmed icon, a reconnect button, a pulse animation when it comes back online. The app doesn't scream at you. It calmly informs and offers resolution.

### Anti-Patterns in Existing RGB Software

Proprietary RGB tools tend to suffer from a specific disease: **feature accumulation without information architecture**. Every new feature gets bolted onto the existing UI without asking "where does this live in the user's mental model?" The result is an app where:

- Effects, layouts, devices, and settings are all accessed through different navigation paradigms
- The sidebar changes meaning depending on context
- Premium upsells are interleaved with functional UI
- The effect library mixes browsing, installing, and configuring into one confused flow
- There's no way to see your whole setup at a glance

Hypercolor's antidote: **one navigation model, zero upsells, clear separation of concerns, and a persistent overview.**

---

## 2. Information Architecture

### Site Map

```
Hypercolor Web UI
│
├── 🏠 Dashboard (default landing)
│   ├── System overview: all devices, current effect, FPS, audio level
│   ├── Quick-switch: recent effects (click to activate)
│   ├── Active alerts (disconnected device, high CPU, etc.)
│   └── Mini spatial preview (live LED colors on silhouette)
│
├── ✨ Effects
│   ├── Browse (grid of animated thumbnails)
│   │   ├── Filter: Category / Audio-reactive / Native vs HTML / Favorites
│   │   ├── Search (instant, fuzzy)
│   │   └── Sort: Popular / Recent / Alphabetical
│   ├── Active Effect (detail view)
│   │   ├── Live full-width preview
│   │   ├── Control panel (auto-generated from metadata)
│   │   ├── Effect info (author, description, type)
│   │   └── "Save as Preset" / "Add to Scene"
│   └── Import / Create
│       ├── Drag-and-drop HTML file import
│       └── Link to dev server docs
│
├── 🗺️ Layout
│   ├── Spatial Editor (Three.js / Canvas)
│   │   ├── Drag devices onto 2D canvas
│   │   ├── Resize, rotate, position zones
│   │   ├── Live effect overlay on zones
│   │   └── Grid snap, alignment guides
│   ├── Device Zones panel (list of all mapped zones)
│   └── Presets (save/load layout configurations)
│
├── 🔌 Devices
│   ├── Connected devices (cards with live LED preview)
│   │   ├── Device detail → zone config, firmware info, protocol
│   │   └── Per-device effect override (optional)
│   ├── Discovered devices (available but not added)
│   ├── Manual add (IP/port for WLED, OpenRGB)
│   └── Backend status (OpenRGB bridge, WLED, HID, Hue)
│
├── 🎭 Scenes & Profiles
│   ├── Scenes (named configurations: effect + layout + device overrides)
│   │   ├── Create / Edit / Delete
│   │   └── Quick-switch buttons
│   ├── Schedules (time-based scene switching)
│   └── Triggers (event-based: audio threshold, app launch, HA webhook)
│
├── 🎤 Inputs
│   ├── Audio source (select device, visualize spectrum)
│   ├── Screen capture (select monitor, preview region)
│   ├── Keyboard state (reactive key visualization)
│   └── MIDI (map controllers to effect parameters)
│
└── ⚙️ Settings
    ├── General (theme, language, startup behavior)
    ├── Performance (target FPS, preview quality, WebSocket rate)
    ├── Network (daemon port, allowed origins, TLS)
    ├── Integrations (Home Assistant URL, OBS WebSocket)
    ├── Advanced (debug logging, effect dev server, raw API)
    └── About (version, system info, links)
```

### Navigation Depth Rules

| Depth | Where | Example |
|-------|-------|---------|
| 0 | Top-level section | Dashboard, Effects, Layout, Devices, Scenes, Settings |
| 1 | Section view | Effects → Browse, Effects → Active |
| 2 | Detail / Editor | Effects → Active → Control Panel |
| 3 | **Maximum** | Settings → Integrations → Home Assistant Config |

**Hard rule: nothing is ever more than 3 clicks from the dashboard.** If we find ourselves nesting deeper, it means the IA is wrong and needs restructuring.

### First Launch vs. Return Visit

**First launch** (no devices configured):
→ Redirects to the **Setup Wizard** (see Section 7: First-Run Experience)

**Return visit** (devices configured):
→ Lands on **Dashboard** showing current state. The last-active effect is running, devices are live, everything is where you left it. Zero clicks to "resume."

**Return visit** (devices configured, daemon was restarted):
→ Lands on **Dashboard** with the last-saved profile auto-loaded. Brief "Reconnecting..." animation as backends come online. Devices appear one by one as they connect.

---

## 3. Navigation Model

### Primary Navigation: Fixed Sidebar

A narrow (56px collapsed / 220px expanded) sidebar on the left. Always visible. Never changes.

```
┌────┬──────────────────────────────────────────────────────┐
│ ◈  │                                                      │
│    │           Main Content Area                          │
│ 🏠 │                                                      │
│    │                                                      │
│ ✨ │                                                      │
│    │                                                      │
│ 🗺️ │                                                      │
│    │                                                      │
│ 🔌 │                                                      │
│    │                                                      │
│ 🎭 │                                                      │
│    │                                                      │
│ 🎤 │                                                      │
│    │                                                      │
│    │                                                      │
│    │                                                      │
│ ⚙️ │                                                      │
└────┴──────────────────────────────────────────────────────┘
```

- **Collapsed by default** on screens < 1440px, expanded on wider screens
- Hover to expand temporarily, click to pin
- Each icon has a tooltip; expanded state shows icon + label
- Active section is highlighted with an `#e135ff` left border accent
- The sidebar is the **only** navigation element. No tabs, no breadcrumbs in the top bar, no hamburger menus. One nav, one truth.

### Secondary Navigation: Section Tabs

Within each top-level section, horizontal tabs sit just below the header:

```
┌────────────────────────────────────────────────────────────┐
│  ✨ Effects                                        [🔍] [⌘K]│
│  ┌──────────┬──────────┬──────────┐                        │
│  │ Browse   │ Active   │ Import   │                        │
│  └──────────┴──────────┴──────────┘                        │
│                                                             │
│  [content area]                                             │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

Tabs are static per section. They don't multiply, nest, or change order. Maximum 4 tabs per section.

### Tertiary Navigation: Command Palette

`Ctrl+K` opens a Spotlight-style command palette:

```
┌──────────────────────────────────────────────────┐
│  🔍 Type a command or search...                  │
├──────────────────────────────────────────────────┤
│  Effects                                         │
│    ◆ Rainbow Wave                                │
│    ◆ Aurora                                      │
│    ◆ Neon Shift                                  │
│  Actions                                         │
│    → Switch to Layout Editor                     │
│    → Reconnect all devices                       │
│    → Toggle audio input                          │
│  Scenes                                          │
│    🎭 Gaming Mode                                │
│    🎭 Chill                                      │
│    🎭 Stream Setup                               │
└──────────────────────────────────────────────────┘
```

The command palette:
- Searches effects, devices, scenes, and actions simultaneously
- Supports fuzzy matching ("rnbw" matches "Rainbow Wave")
- Shows keyboard shortcuts next to actions
- Remembers recent commands (MRU ordering)
- Supports slash commands for power users: `/set aurora`, `/device wled`, `/scene gaming`

### Keyboard Shortcuts

| Action | Shortcut | Context |
|--------|----------|---------|
| Command palette | `Ctrl+K` | Global |
| Go to Dashboard | `Ctrl+1` | Global |
| Go to Effects | `Ctrl+2` | Global |
| Go to Layout | `Ctrl+3` | Global |
| Go to Devices | `Ctrl+4` | Global |
| Go to Scenes | `Ctrl+5` | Global |
| Search effects | `/` | Effects section |
| Next effect | `→` or `J` | Effect browser |
| Previous effect | `←` or `K` | Effect browser |
| Activate effect | `Enter` | Effect browser |
| Toggle sidebar | `[` | Global |
| Quick scene switch | `Ctrl+Shift+1-9` | Global |
| Toggle audio reactive | `A` | Active effect |
| Fullscreen preview | `F` | Active effect |
| Undo (layout changes) | `Ctrl+Z` | Layout editor |

### Breadcrumbs

Shown in the header bar, but purely for orientation — not the primary way to navigate back:

```
Dashboard  >  Effects  >  Aurora
```

The breadcrumb uses `>` separators, muted text, and clicking any segment navigates there. But since the sidebar is always visible, breadcrumbs exist as a "you are here" marker, not a "go back" mechanism.

---

## 4. Visual Language

### The SilkCircuit Neon Palette — Applied to Lighting

The core palette from the SilkCircuit design system, with semantic assignments specific to Hypercolor:

| Color | Hex | Semantic Role |
|-------|-----|---------------|
| Electric Purple | `#e135ff` | Active state, selected items, primary actions, brand accent |
| Neon Cyan | `#80ffea` | Interactive elements, links, hover states, device online |
| Coral | `#ff6ac1` | Effect thumbnails border, audio visualization, secondary accent |
| Electric Yellow | `#f1fa8c` | Warnings, attention badges, pending states |
| Success Green | `#50fa7b` | Connected devices, success confirmations, healthy status |
| Error Red | `#ff6363` | Disconnected devices, errors, critical alerts |

### Surface System

Dark UI with layered depth. No pure black. No pure white.

```
Layer 0 (Deepest):   #0a0a0f   — App background, behind everything
Layer 1 (Base):      #12121a   — Main content area background
Layer 2 (Cards):     #1a1a26   — Card surfaces, sidebar background
Layer 3 (Elevated):  #222233   — Modals, dropdowns, tooltips
Layer 4 (Float):     #2a2a3d   — Hover states, active cards
Border (Subtle):     #2d2d44   — Card borders, dividers (very low contrast)
Border (Active):     #e135ff33 — Active/selected item border (purple at 20% opacity)
```

Why not pure black (#000): The contrast ratio between neon accents and pure black exceeds comfortable viewing, especially in dark rooms — which is exactly where people use RGB software. `#0a0a0f` has the same "dark room" feel without the harshness.

### Typography

```
Font Stack:
  Headings:  "JetBrains Mono", "Fira Code", monospace
  Body:      "Inter", -apple-system, "Segoe UI", sans-serif
  Code/Data: "JetBrains Mono", "Fira Code", monospace

Sizes:
  xs:  0.75rem (12px)  — metadata, timestamps
  sm:  0.875rem (14px) — body text, controls
  base: 1rem (16px)    — section content
  lg:  1.25rem (20px)  — section headers
  xl:  1.5rem (24px)   — page titles
  2xl: 2rem (32px)     — hero numbers (FPS counter, device count)
```

Monospace for headings and data is intentional. This is a tool for technical users. The monospace aesthetic reinforces "precision instrument" and aligns with the SilkCircuit design language already established across the Hypercolor ecosystem.

### The Preview-vs-Chrome Problem

The single hardest visual design challenge in a lighting control app: **the effect preview uses color as content, and the UI uses color as interaction signifiers.** They must not compete.

**Solution: Containment and Suppression**

```
┌─────────────────────────────────────────────────────────────┐
│  ┌─────────────────────────────────────────────────┐        │
│  │                                                  │        │
│  │           EFFECT PREVIEW AREA                    │        │
│  │     (vivid, full saturation, alive)              │        │
│  │                                                  │        │
│  │     This is where color lives.                   │        │
│  │                                                  │        │
│  └─────────────────────────────────────────────────┘        │
│                                                              │
│  ┌─ Controls ──────────────────────────────────────┐        │
│  │  Speed  ████████░░░░  50                        │        │
│  │  Hue    ████████████░  280                      │        │
│  │  Scale  ██░░░░░░░░░░  15                        │        │
│  └─────────────────────────────────────────────────┘        │
│                                                              │
│  UI chrome is muted. Desaturated. Low-key.                  │
│  Only the preview and active control accents use vivid color.│
└─────────────────────────────────────────────────────────────┘
```

**Rules:**
1. UI accent colors (purple, cyan) appear at **reduced opacity** (50-70%) except for the focused/active element
2. The effect preview area has **no visible border** — it bleeds to the edge of its container, with a subtle 1px `#2d2d44` outline
3. Control sliders use a thin track with a small accent-colored thumb, not thick gradient-filled bars
4. The sidebar uses icon-only mode by default, minimizing chrome surface area
5. When a full-screen preview is active (`F` key), all UI fades to 0% opacity on a 300ms ease, leaving only the preview

### Glassmorphism — Tasteful, Not Gimmicky

Used sparingly for overlays that sit on top of the preview:

```css
.glass-overlay {
  background: rgba(10, 10, 15, 0.72);
  backdrop-filter: blur(16px) saturate(120%);
  border: 1px solid rgba(225, 53, 255, 0.08);
  border-radius: 12px;
}
```

Where it's used:
- Command palette overlay
- Toast notifications
- Tooltips
- The control panel when overlaid on fullscreen preview mode

Where it's NOT used:
- Cards (too many glass surfaces = soup)
- Sidebar (needs to be solid and reliable)
- Modals (they dim the background; glass on dimmed content is pointless)

### Alive Without Being Annoying

The UI breathes. But it doesn't hyperventilate.

- **Active effect indicator:** A subtle 4px gradient line below the header animates with the current effect's dominant colors. Changes slowly (2s transition). This is the app's "heartbeat."
- **Device status dots:** 6px circles next to device names. Online = `#50fa7b` with a barely-perceptible 3s pulse. Offline = `#ff6363` static. No animation on offline — stillness communicates "dead."
- **Sidebar active indicator:** 3px `#e135ff` bar on the left edge of the active nav item. Slides smoothly when switching sections (200ms ease-out).
- **Background canvas:** The Layer 0 background renders an extremely faint (2-3% opacity), slowed-down version of the active effect. Just enough to feel alive. Disabled if `prefers-reduced-motion` is set.

---

## 5. Responsive Design

### Breakpoints

| Name | Width | Context | Layout |
|------|-------|---------|--------|
| `desktop-xl` | >= 1920px | Full setup, multi-monitor | 3-column: sidebar + main + inspector |
| `desktop` | 1440-1919px | Standard desktop | 2-column: sidebar (collapsed) + main (stacked) |
| `tablet` | 768-1439px | iPad, Surface, couch browsing | Bottom tab bar + full-width main |
| `phone` | < 768px | Phone, quick switching | Bottom tab bar + card-based flow |

### Desktop XL (>= 1920px)

The power user layout. Three persistent columns.

```
┌────┬──────────────────────────────────┬─────────────────┐
│ ◈  │                                  │ Inspector Panel  │
│    │                                  │                  │
│ 🏠 │      Main Content Area           │ ┌──────────────┐│
│    │                                  │ │ Effect Info   ││
│ ✨ │  ┌──────────────────────────┐    │ │              ││
│    │  │  Live Preview            │    │ │ Author: ...  ││
│ 🗺️ │  │  (large, full-width)     │    │ │ Type: WebGL  ││
│    │  │                          │    │ └──────────────┘│
│ 🔌 │  └──────────────────────────┘    │                  │
│    │                                  │ ┌──────────────┐│
│ 🎭 │  ┌──────────────────────────┐    │ │ Controls     ││
│    │  │  Effect Grid / Detail    │    │ │ Speed: ███░  ││
│ 🎤 │  │                          │    │ │ Hue: █████░  ││
│    │  └──────────────────────────┘    │ └──────────────┘│
│    │                                  │                  │
│ ⚙️ │                                  │ ┌──────────────┐│
│    │                                  │ │ Device Map   ││
│    │                                  │ │ (mini)       ││
│    │                                  │ └──────────────┘│
└────┴──────────────────────────────────┴─────────────────┘
```

The inspector panel shows contextual detail for whatever is selected in the main area: effect controls when browsing effects, device info when viewing devices, zone properties when editing the layout.

### Desktop (1440-1919px)

Sidebar collapses to icons. Inspector panel folds into the main area as a slide-in panel or stacked below the preview.

```
┌──┬──────────────────────────────────────────────────────┐
│◈ │                                                      │
│🏠│      Main Content Area                               │
│✨│                                                      │
│🗺️│  ┌──────────────────────────────────────────────┐    │
│🔌│  │  Live Preview                                │    │
│🎭│  └──────────────────────────────────────────────┘    │
│🎤│                                                      │
│  │  ┌─────────────────┬────────────────────────────┐    │
│  │  │ Effect Grid     │ Controls (slide-in)        │    │
│⚙️│  │                 │                            │    │
│  │  └─────────────────┴────────────────────────────┘    │
└──┴──────────────────────────────────────────────────────┘
```

### Tablet (768-1439px)

Sidebar becomes a bottom tab bar (5 items — Dashboard, Effects, Layout, Devices, More). The "More" item opens a sheet with Scenes, Inputs, Settings.

```
┌────────────────────────────────────────────────────────────┐
│  ← Effects                                    [🔍]         │
│────────────────────────────────────────────────────────────│
│                                                            │
│  ┌──────────────────────────────────────────────────┐      │
│  │  Live Preview (full width)                       │      │
│  └──────────────────────────────────────────────────┘      │
│                                                            │
│  ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐            │
│  │ ████ │ │ ████ │ │ ████ │ │ ████ │ │ ████ │            │
│  │Effect│ │Effect│ │Effect│ │Effect│ │Effect│            │
│  │ 1    │ │ 2    │ │ 3    │ │ 4    │ │ 5    │            │
│  └──────┘ └──────┘ └──────┘ └──────┘ └──────┘            │
│                                                            │
│────────────────────────────────────────────────────────────│
│  🏠    ✨    🗺️    🔌    •••                               │
└────────────────────────────────────────────────────────────┘
```

Controls open as a bottom sheet (drag up to expand). Swipe left/right to browse effects. Tap to activate.

### Phone (< 768px)

**Purpose-limited.** The phone UI is a remote control, not a configuration tool. You don't set up your spatial layout on a phone.

```
┌──────────────────────────┐
│  Hypercolor         [⚡]  │
│──────────────────────────│
│                          │
│  ┌──────────────────┐    │
│  │ Current Effect   │    │
│  │ ╔══════════════╗ │    │
│  │ ║  Live Preview ║ │    │
│  │ ╚══════════════╝ │    │
│  │ Aurora        ▶️  │    │
│  └──────────────────┘    │
│                          │
│  ┌──────────────────┐    │
│  │ Quick Scenes     │    │
│  │ [Gaming] [Chill] │    │
│  │ [Stream] [Movie] │    │
│  └──────────────────┘    │
│                          │
│  Speed  ████████░░  50   │
│  Hue    ██████░░░░  200  │
│                          │
│──────────────────────────│
│  🏠    ✨    🔌    ⚙️     │
└──────────────────────────┘
```

Phone features:
- View current effect + live preview
- Switch effects (simplified grid, large touch targets)
- Activate saved scenes (one-tap)
- Adjust current effect's primary controls
- View device status

Phone non-features (require tablet/desktop):
- Spatial layout editing
- Device configuration
- Schedule creation
- Effect importing
- Advanced settings

---

## 6. Accessibility

### The Dark Neon Accessibility Challenge

A dark UI with neon accents is a minefield. Electric purple on near-black technically passes WCAG AA (the contrast ratio of `#e135ff` on `#0a0a0f` is approximately 5.2:1), but neon cyan on dark (`#80ffea` on `#0a0a0f`) is roughly 12.5:1 — no problem. The danger zone is **neon on neon** (e.g., purple text on a dark purple card) and **small text** (where AA requires 4.5:1 minimum).

### Contrast Guarantees

| Element | Foreground | Background | Min Ratio | WCAG |
|---------|-----------|-----------|-----------|------|
| Body text | `#e0e0e8` | `#12121a` | 13.2:1 | AAA |
| Secondary text | `#8888a0` | `#12121a` | 5.1:1 | AA |
| Interactive accent | `#e135ff` | `#12121a` | 5.2:1 | AA |
| Link/hover | `#80ffea` | `#12121a` | 12.5:1 | AAA |
| Error on card | `#ff6363` | `#1a1a26` | 5.4:1 | AA |
| Warning text | `#f1fa8c` | `#12121a` | 14.6:1 | AAA |

**Rule:** No text element may fall below 4.5:1 contrast ratio against its immediate background. Interactive elements (buttons, links) must meet AA (4.5:1) minimum. All ratios validated with automated tooling in CI.

### Focus Indicators

Every focusable element gets a visible focus ring:

```css
:focus-visible {
  outline: 2px solid #80ffea;
  outline-offset: 2px;
  border-radius: 4px;
}
```

Neon cyan ring — unmissable, on-brand, and high contrast against every surface in our layer system.

### High Contrast Mode

Activated via Settings or system preference detection (`prefers-contrast: more`):

- Surface layers increase separation: `#000000`, `#0f0f0f`, `#1a1a1a`, `#262626`
- Accent colors shift to full brightness, no transparency
- All borders become solid `#ffffff33`
- Text shifts to pure white `#ffffff`
- Minimum contrast rises to 7:1 (WCAG AAA)

### Reduced Motion

Detected via `prefers-reduced-motion: reduce`:

- Background canvas effect (the subtle ambient glow) disables completely
- Effect thumbnail animations freeze on first frame (show static preview)
- Sidebar transitions become instant (no slide)
- Device status pulse animations stop
- Page transitions are instant cross-fades (no directional movement)
- Effect preview still animates (it's content, not decoration)

### Screen Reader Support

This is a fundamentally visual app. A screen reader user can't "see" that their LEDs are purple. But they can:

- **Navigate the full IA** with ARIA landmarks: `nav` (sidebar), `main` (content), `region` (panels)
- **Know what effect is active:** "Current effect: Aurora. Audio-reactive: yes. 60 FPS."
- **Control parameters:** Every slider, toggle, and dropdown has `aria-label`, `aria-valuemin`, `aria-valuemax`, `aria-valuenow`
- **Know device status:** "WLED Strip 1: connected, 120 LEDs, zone: desk-back"
- **Receive live updates:** `aria-live="polite"` regions announce device connect/disconnect, effect changes, errors
- **Navigate effect browser:** Each effect card announces its name, author, category, and "press Enter to activate"

The spatial layout editor is the hardest to make accessible. We use an `aria-describedby` panel that narrates zone positions as a list: "Zone: WLED Strip 1. Position: upper left. Size: 40% width, 20% height. 120 LEDs." Users can reposition zones via keyboard arrow keys with spoken position feedback.

### Color Blindness

Effect previews are inherently color-dependent — we can't change what the user's LEDs display. But the UI itself never relies on color alone to convey state:

| State | Color | Also Uses |
|-------|-------|-----------|
| Connected | Green dot | "Connected" text + checkmark icon |
| Disconnected | Red dot | "Disconnected" text + X icon |
| Warning | Yellow badge | Triangle icon + descriptive text |
| Active nav item | Purple bar | Bold text + filled icon (vs outline) |
| Selected effect | Purple border | Checkmark overlay + "Active" badge |

---

## 7. First-Run Experience

### The Zero-to-Wow Pipeline

The goal: the user goes from "I just installed this" to "my room looks incredible" in under 5 minutes. Not 5 minutes of reading. 5 minutes of doing.

### Step 1: Welcome (5 seconds)

```
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│                       ◈ Hypercolor                           │
│                                                              │
│            Open-source RGB orchestration for Linux           │
│                                                              │
│                                                              │
│                  [ Let's light it up → ]                     │
│                                                              │
│                                                              │
│  The Hypercolor logo pulses through a slow SilkCircuit       │
│  gradient — the first thing the user sees IS a lighting      │
│  effect. We're showing, not telling.                         │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

One button. No login. No account. No terms. Let's go.

### Step 2: Device Discovery (30-60 seconds)

```
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│  🔍 Scanning for RGB devices...                              │
│                                                              │
│  ┌────────────────────────────────────────────────────────┐  │
│  │  ✅ WLED - Living Room Strip     192.168.1.42  120 LEDs│  │
│  │  ✅ WLED - Desk Backlight        192.168.1.43   60 LEDs│  │
│  │  ⏳ OpenRGB (connecting to localhost:6742...)           │  │
│  │  ✅ OpenRGB - ASUS Z790-A AURA           8 zones       │  │
│  │  ✅ OpenRGB - G.Skill DDR5 RGB           2 sticks      │  │
│  │  ✅ PrismRGB Prism 8                     8 channels    │  │
│  │  ✅ PrismRGB Prism S                     2 strimers    │  │
│  └────────────────────────────────────────────────────────┘  │
│                                                              │
│  Found 7 devices with 1,428 LEDs                             │
│                                                              │
│  [ + Add device manually ]         [ Continue → ]            │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

What happens automatically:
- mDNS scan for WLED devices
- TCP probe for OpenRGB on localhost:6742
- USB HID enumeration for PrismRGB / Nollie devices
- Hue bridge discovery via mDNS

Devices appear in real-time as they're found. Each one lights up with a brief flash on the physical hardware (a quick white pulse) so the user can confirm "yes, that's my desk strip." This confirmation flash is the first "whoa" moment.

### Step 3: Quick Layout (60-90 seconds)

```
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│  Where are your devices?                                     │
│  Drag them roughly where they are in your setup.             │
│  (You can fine-tune this later.)                             │
│                                                              │
│  ┌────────────────────────────────────────────────────────┐  │
│  │                                                        │  │
│  │       [ Monitor ]                                      │  │
│  │    ╔═══════════════╗                                   │  │
│  │    ║               ║     ┌─────────────┐               │  │
│  │    ║               ║     │  Prism 8    │               │  │
│  │    ║               ║     │  ch1-ch4    │               │  │
│  │    ╚═══════════════╝     └─────────────┘               │  │
│  │    [▓▓▓ Desk Strip ▓▓▓]                                │  │
│  │                                                        │  │
│  │    [  WLED Strip  ]    [ Strimer ]                     │  │
│  │                                                        │  │
│  └────────────────────────────────────────────────────────┘  │
│                                                              │
│  Devices to place:  Prism S #2                               │
│                                                              │
│  [ Skip for now ]                 [ Continue → ]             │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

This is a simplified version of the full spatial editor. Pre-made silhouettes for common setups (monitor, desk, PC case). Devices snap to zones. The user doesn't need to be precise — approximate placement still creates a good effect-to-hardware mapping.

### Step 4: Pick Your First Effect (15 seconds)

```
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│  Pick a vibe.                                                │
│                                                              │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐       │
│  │ ▓▓▓▓▓▓▓▓ │ │ ░▒▓▓▒░░░ │ │ ▓░▓░▓░▓░ │ │ ▓▓▓▓▓▓▓▓ │       │
│  │ ▓▓▓▓▓▓▓▓ │ │ ░░▒▓▓▒░░ │ │ ░▓░▓░▓░▓ │ │ ▓▓▒░░▒▓▓ │       │
│  │  Aurora   │ │  Rainbow │ │  Neon    │ │  Pulse   │       │
│  │          │ │  Wave    │ │  Shift   │ │          │       │
│  └──────────┘ └──────────┘ └──────────┘ └──────────┘       │
│                                                              │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐       │
│  │ ████████ │ │ ░░░░░░░░ │ │ ▓▒░▒▓░▒▓ │ │ ████████ │       │
│  │ ████████ │ │ ████████ │ │ ▓░▒▓░▒▓░ │ │ ░░░░░░░░ │       │
│  │  Solid   │ │  Breath  │ │  Plasma  │ │  Side to │       │
│  │  Color   │ │          │ │          │ │  Side    │       │
│  └──────────┘ └──────────┘ └──────────┘ └──────────┘       │
│                                                              │
│  Each thumbnail is a LIVE animated preview, not a            │
│  static image. Click one and it activates immediately.       │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

8 curated starter effects. No categories, no filters, no noise. Just "pick one." The moment they click, their real physical LEDs activate with that effect. This is the "holy shit" moment.

### Step 5: You're In

Redirect to the Dashboard. The selected effect is running. All devices are mapped. A subtle toast appears:

```
  ┌─────────────────────────────────────────────────┐
  │  ✅  Setup complete. 7 devices, 1,428 LEDs.     │
  │  Explore effects, refine your layout, or just   │
  │  enjoy the glow.                                 │
  └─────────────────────────────────────────────────┘
```

Total time: ~3 minutes. Total "reading": ~4 sentences. Total "holy shit" moments: at least 2 (the device flash and the first effect activation).

---

## 8. Persona-Driven Walkthroughs

### Bliss — The Power User

**Setup:** 12 RGB devices (ASUS mobo, GPU, DDR5, Corsair AIO, 2x Razer peripherals, 3x PrismRGB controllers, 2x WLED strips, Dygma Defy). Custom-authored effects. Writes WGSL shaders. Wants everything keyboard-driven.

**Her Hypercolor session:**

1. Opens browser to `localhost:9420`. Dashboard loads in <200ms. She sees all 12 devices green, Aurora running at 60fps, audio reactive to Spotify.

2. `Ctrl+K` → types "hyper" → selects "ADHD Hyperfocus" → effect activates. Total time: 2 seconds.

3. The control panel slides in on the right. She adjusts Focus Radius (28→45), Peripheral Blur (60→90), and Energy (120→150) using arrow keys on the focused sliders. Each adjustment reflects immediately on her real LEDs.

4. `Ctrl+S` → "Save as Preset" → names it "deep work" → saved. She can now switch to this exact configuration from the command palette or CLI.

5. Switches to Layout Editor (`Ctrl+3`). Drags her new WLED strip to the position behind her second monitor. Uses Shift+arrow keys for pixel-precise positioning. Rotates it 90 degrees with `R` key.

6. Opens a second terminal: `hypercolor set plasma --device "WLED Desk"` — overrides just one device while the rest keep running Aurora. The CLI and web UI are in sync instantly via WebSocket.

7. Creates a Scene called "Stream Setup" that combines her Twitch effect on peripherals, ambient glow on WLED strips, and static purple on case lighting. Binds it to `Ctrl+Shift+3`.

**What makes Bliss happy:**
- Command palette is the primary interface — she rarely touches the sidebar
- Every action has a keyboard shortcut
- CLI and web UI are interchangeable
- No "are you sure?" dialogs — she's sure
- WebSocket preview updates at 60fps, zero perceptible lag between slider movement and LED change

### Jake — The Gamer

**Setup:** Gaming PC with motherboard RGB, GPU RGB, RAM RGB, and a single WLED strip behind his desk. Installed Hypercolor because his friend told him it was "like having real RGB control on Linux." Has never written code.

**His Hypercolor session:**

1. First launch. The setup wizard finds 4 devices. He drags them roughly into position on the simplified layout editor. Picks "Rainbow Wave" as his first effect. His room immediately looks awesome.

2. Two weeks later, he wants to try something new. Opens Effects (clicks the sparkle icon in the sidebar). The grid shows animated thumbnails. He scrolls. Sees "Cyberpunk 2077" — the preview looks sick. Clicks it. Done.

3. Notices a "Audio Reactive" filter tag on some effects. Toggles it. Browses effects that pulse with his music. Picks "Pump Up Beats." His WLED strip is going nuts behind his monitor while he games. He's happy.

4. Wants to save two configurations: one for gaming (intense, reactive) and one for chilling (slow, ambient). Goes to Scenes. Creates "Gaming" and "Chill." Now he switches between them from the Dashboard's quick-switch cards.

5. Never touches the Layout Editor again after initial setup. Never opens Settings. Never uses the keyboard shortcuts. And that's perfectly fine.

**What makes Jake happy:**
- Animated thumbnails — he can see what an effect does before activating it
- One-click activation — no configuration needed for most effects
- The "Audio Reactive" filter — he didn't know he wanted this until he saw it
- Scenes with quick-switch — two taps from Dashboard
- The app never shows him anything intimidating (no JSON, no code, no API endpoints)

### Luna — The Streamer

**Setup:** 6 RGB devices including Razer peripherals and WLED strips behind her camera setup. Needs instant scene switching for stream transitions. Uses OBS on the same machine.

**Her Hypercolor session:**

1. Configures three Scenes: "Pre-Stream" (calm purple ambient), "Live" (energetic reactive lighting matching her brand colors), and "BRB" (slow breathing, dimmed).

2. In Settings → Integrations, connects Hypercolor to OBS via OBS WebSocket. Configures triggers:
   - OBS scene "Starting Soon" → Hypercolor scene "Pre-Stream"
   - OBS scene "Main Camera" → Hypercolor scene "Live"
   - OBS scene "BRB" → Hypercolor scene "BRB"

3. During her stream: she switches OBS scenes and her entire room lighting changes simultaneously. Zero manual intervention. Her chat goes wild.

4. Mid-stream, she opens Hypercolor on her phone (same `localhost:9420` but on her phone's browser via local network). She's on the couch for a "chill chat" segment and switches to a calm effect without reaching for her PC. The phone UI shows her Quick Scenes as large, easy-to-tap cards.

5. Post-stream: she creates a new effect in Hypercolor's effect browser — imports a custom HTML effect that matches her brand gradient (she got it from a viewer who made it for her). Drag, drop, it's in her library.

**What makes Luna happy:**
- OBS integration is a first-class feature, not a hack
- Scene switching is instant (< 100ms perceptible transition)
- The phone UI is a real remote control, not a shrunken desktop app
- Importing effects is drag-and-drop, not "copy to this folder and restart"
- She never sees technical details about DDP packets or USB HID

### Dev — The Plugin Author

**Setup:** Developing a custom device backend for an obscure LED controller he found on AliExpress. Needs debug visibility into the protocol layer.

**His Hypercolor session:**

1. Reads the developer docs at `localhost:9420/docs` (embedded, accessible from the Settings → Advanced panel). The docs cover the Wasm plugin WIT interface, the REST API, and the WebSocket protocol.

2. Opens Settings → Advanced → Debug Mode. This reveals:
   - A live event log (all `HypercolorEvent` bus traffic, color-coded)
   - Per-backend frame timing (how long each `push_frame` takes)
   - Raw WebSocket frame inspector
   - Effect engine stats (render time, canvas readback time)

3. Writes his device backend as a Wasm module using the WIT interface. Drops the `.wasm` file into `~/.config/hypercolor/plugins/`. The daemon hot-reloads it. His new device appears in the Devices panel with a "Plugin" badge.

4. The debug view shows him exactly what color data is being sent to his device each frame, the latency of his `push_frame` implementation, and any errors his Wasm module logs.

5. His device backend has a bug — it's dropping every 3rd frame. The debug panel's frame timing graph shows the regular spikes. He fixes the buffer allocation in his Wasm code, the hot-reload picks it up, the graph smooths out.

**What makes Dev happy:**
- Debug mode is opt-in but comprehensive
- The Wasm plugin lifecycle is drop-in, no restart required
- API documentation is embedded in the app, not on a separate website
- The event bus is inspectable — he can see exactly what the daemon is doing
- Frame timing visualization makes performance issues immediately visible

### Alex — The Smart Home Person

**Setup:** Home Assistant controls her whole house. She has WLED strips in every room, Hue bulbs, and some OpenRGB devices in her office. She wants Hypercolor to be part of her automation stack, not a standalone island.

**Her Hypercolor session:**

1. In Settings → Integrations, she enters her Home Assistant URL and creates a long-lived access token. Hypercolor now appears as an integration in HA.

2. She creates Scenes in Hypercolor for different moods: "Morning" (warm sunrise gradient), "Focus" (cool blue, static), "Movie" (dim ambient, screen capture mode), "Party" (audio reactive, max energy).

3. In Home Assistant, she creates automations:
   - Sunrise → trigger Hypercolor "Morning" scene
   - HA scene "Movie Night" → trigger Hypercolor "Movie" + dim all Hue lights
   - Spotify playing + volume > 50% → trigger Hypercolor "Party"
   - 11 PM → trigger Hypercolor "Chill" (gentle breathing, low brightness)

4. She configures Hypercolor Schedules as a backup (in case HA is down):
   - Weekdays 7 AM: "Morning"
   - Weekdays 9 AM: "Focus"
   - Every day 10 PM: "Chill"

5. On her phone, she has an HA dashboard widget that shows the current Hypercolor scene and lets her switch between them. Hypercolor's own phone UI is her fallback.

**What makes Alex happy:**
- Home Assistant integration is bidirectional (HA can trigger Hypercolor AND read its state)
- Schedules work independently of HA (defense in depth)
- Hypercolor exposes its scenes as HA entities — she can use them in any automation
- The webhook trigger system lets her connect to IFTTT, Node-RED, or anything that speaks HTTP
- Device management is centralized in Hypercolor, not split between WLED app + OpenRGB + Hue app

---

## 9. Anti-Patterns in Existing RGB Software

### Anti-Pattern 1: The Shape-Shifting Sidebar

**The problem:** The left sidebar changes its content depending on what mode you're in. Sometimes it shows devices, sometimes effects categories, sometimes settings groups. The user's spatial memory is constantly invalidated.

**Hypercolor's fix:** The sidebar is a fixed list of 7 top-level sections. It never changes. The icons never move. If you close your eyes and click the third icon, it's always the Layout Editor. Spatial consistency is sacred.

### Anti-Pattern 2: The Marketplace Invasion

**The problem:** Free effects, premium effects, downloadable effects, and installed effects are all mixed together in the same browsing experience. Ads and upsell prompts interrupt the functional workflow. Users report having to "scroll to the bottom" to find free effects, past walls of premium content.

**Hypercolor's fix:** There is no marketplace. There is no premium tier. All effects are yours. The effect browser shows what you have. Import adds to what you have. Community effect repositories are linked but never injected into your browsing flow. Zero ads, zero upsells, zero "upgrade to unlock."

### Anti-Pattern 3: The Invisible Effect Preview

**The problem:** Effects are often represented by a static thumbnail or just a name. The user can't tell what an effect actually looks like until they apply it. Even the community store shows effects as static screenshots with separate video links.

**Hypercolor's fix:** Every effect thumbnail is a live animated canvas. The effect is literally running in miniature, at reduced framerate (15fps for thumbnails, to keep GPU usage sane when showing a grid of 50+). Hover to see it at full 30fps. Click to activate at 60fps. The progression is: tiny live preview → hover expands → click activates globally. You never wonder "what does this look like?"

### Anti-Pattern 4: Configuration Whack-a-Mole

**The problem:** Device configuration, effect parameters, layout positioning, and profile management all live in different places with different interaction paradigms. Users report that "the UI is so confusing when trying to assign settings." Finding where a specific setting lives requires trial and error.

**Hypercolor's fix:** The Inspector Panel pattern. Select anything — an effect, a device, a zone — and its configuration appears in a consistent right panel. Always the same position, always the same interaction pattern (labeled controls, sliders, dropdowns, toggles). The user learns one interaction model and applies it everywhere.

### Anti-Pattern 5: Startup Confusion

**The problem:** First launch drops the user into a complex interface with no guidance. Device detection happens silently. The relationship between effects, devices, and layouts is unclear. Users report that the app "takes getting used to."

**Hypercolor's fix:** The setup wizard (Section 7). But also: Hypercolor has a clear conceptual model that the UI reinforces:

```
Effects produce color → Layout maps color to positions → Devices display the colors
```

This pipeline is reflected in the sidebar order (Effects → Layout → Devices) and in the Dashboard's overview (which shows the pipeline as a visual flow: effect preview → spatial map → device status). The user understands the system by looking at the Dashboard.

### Anti-Pattern 6: Mobile Afterthought

**Common anti-pattern:** Windows-only, no remote control capability. If your PC is across the room, you walk to it.

**Hypercolor's fix:** It's a web app. Any device on your network can access it. The responsive design ensures it's genuinely useful on a phone (Section 5), not just "the same UI but smaller."

### Anti-Pattern 7: The Dead State Problem

**The problem:** When these tools crash or are closed, your devices either freeze on their last color or revert to their hardware default (which might be a blinding rainbow). There's no graceful degradation.

**Hypercolor's fix:** The daemon runs as a systemd service. If the web UI disconnects, nothing changes — the daemon keeps running. If the daemon restarts, device backends execute their shutdown sequence (hardware fallback colors configurable per-device). If a single device disconnects, the rest of the system continues unaffected. Graceful degradation at every level.

### Anti-Pattern 8: No Keyboard Workflow

**The problem:** Everything requires mouse clicks through nested menus. No keyboard shortcuts, no command palette, no CLI integration.

**Hypercolor's fix:** Command palette, comprehensive keyboard shortcuts, and a full CLI that can do everything the web UI can. Power users might never open the web UI at all — `hypercolor set aurora && hypercolor profile gaming` from a terminal is a valid workflow.

---

## 10. Microinteractions

### Effect Switching

When the user activates a new effect, the transition should feel intentional, not jarring.

**Crossfade (default):** The active effect canvas cross-fades to the new effect over 300ms. On the physical LEDs, this means the color buffer smoothly interpolates between the old and new frame data. Users can configure the transition duration (0-2000ms) or disable transitions entirely.

```
Frame N:   100% old effect
Frame N+6:  66% old + 33% new
Frame N+12: 33% old + 66% new
Frame N+18: 100% new effect
```

In the UI, the effect browser card gets a subtle `#e135ff` border pulse (200ms ease-in, 400ms ease-out) to confirm the activation. The live preview area morphs smoothly.

### Device Connect / Disconnect

**Connect animation:**
1. Device card appears in the Devices panel with 0 opacity, translates up 8px
2. 200ms ease-out: fades in, slides to position
3. Status dot starts as white, transitions to `#50fa7b` green over 400ms
4. If this is the first time seeing the device, a brief "New device" badge appears for 3 seconds
5. On the physical device: a quick white pulse (100ms on, 100ms off, 100ms on) as a "hello, I see you" confirmation

**Disconnect animation:**
1. Status dot transitions from green to `#ff6363` red over 200ms
2. Device card gets a subtle desaturation (CSS filter: `saturate(0.3)`) over 300ms
3. A small "Reconnecting..." label appears below the device name
4. If reconnection succeeds: reverse the desaturation, green dot returns
5. If reconnection fails after 30 seconds: card stays desaturated, "Disconnected" label, manual reconnect button appears

**No popups. No modal alerts. No "DEVICE DISCONNECTED!!!" screaming.** The card calmly updates. The user notices when they look. If they don't look, nothing interrupts them.

### Slider Interaction

Effect control sliders are the most-used microinteraction in the app.

```
Default state:
  Track:  2px height, #2d2d44 (barely visible)
  Thumb:  12px circle, #e135ff (the only vivid element)
  Value:  right-aligned, #8888a0 (muted)

Hover:
  Track:  2px height, #e135ff33 (purple glow appears)
  Thumb:  14px circle, #e135ff (slightly larger)
  Value:  #e0e0e8 (brightens)

Dragging:
  Track:  filled portion glows #e135ff55
  Thumb:  16px circle, #e135ff with box-shadow glow
  Value:  updates in real-time, #80ffea (cyan, the "active" color)
  Canvas: updates every frame as the value changes

Release:
  200ms ease-out back to hover state
  Value: flashes briefly, then settles to muted
```

### Loading States

**Initial page load:** The SilkCircuit logo renders as a glowing line drawing that traces itself (SVG path animation, 1.5s). This replaces a generic spinner.

**Effect loading (Servo renderer, HTML effects):** A shimmer animation on the preview area — a diagonal gradient of `#e135ff11` sweeping left to right every 1.5s. The preview area shows the text "Initializing..." in muted type. When the first frame renders, the shimmer cross-fades to the live preview.

**Device discovery:** Each device slot shows a pulsing skeleton card (the standard "loading placeholder" pattern but in our dark palette). As devices are found, skeleton cards transition to real device cards.

**No spinners.** Spinners are lazy UX. Every loading state communicates what's happening and approximately how long it'll take.

### Error Presentation

Errors are categorized by severity and presented accordingly:

**Info** (e.g., "Effect uses audio but no audio source is configured"):
- Inline yellow banner within the relevant panel
- Includes a direct action: "Configure audio source →"
- Auto-dismisses after 10 seconds or on action

**Warning** (e.g., "OpenRGB bridge is not running"):
- Amber indicator on the Dashboard's system overview
- Persistent until resolved
- Shows in the Devices section as a backend status card

**Error** (e.g., "USB permission denied for PrismRGB"):
- Red banner at the top of the relevant section
- Includes diagnostic info and a fix: "Run: `sudo usermod -aG plugdev $USER`"
- Does NOT block the rest of the app — you can still use other devices

**Fatal** (e.g., "Daemon connection lost"):
- Full-page overlay (glass morphism on dimmed background)
- "Reconnecting..." with a progress indicator
- Auto-reconnects every 2 seconds
- Manual "Connect to different daemon" option

### Toast Notifications

For non-blocking confirmations:

```
┌─────────────────────────────────────────────┐
│  ✅  Scene "Gaming" activated               │
│  12 devices • Rainbow Wave • Audio On       │
└─────────────────────────────────────────────┘
```

- Appear bottom-right
- Glass morphism background
- Auto-dismiss after 4 seconds
- Stack vertically if multiple (max 3 visible, older ones fade)
- Swipe/click to dismiss early
- Semantic icon + color: green for success, yellow for warning, red for error

### Scene Transition

When switching scenes (which may change the effect, layout assignments, and device overrides simultaneously):

1. **Announce:** Toast notification with scene name
2. **Fade:** All device cards briefly dim (100ms, opacity 0.6)
3. **Apply:** New effect starts, layout changes apply
4. **Resolve:** Device cards brighten back as each device acknowledges the new frame data
5. **Confirm:** Dashboard's active scene indicator updates with a subtle slide animation

The staggered device-by-device brightening creates a "cascade" feeling — like the scene is rippling through your setup. It's both aesthetically pleasing and functionally informative (you can see which devices updated and in what order, which is diagnostic information presented as delight).

---

## Appendix A: Component Inventory

Key SvelteKit components that implement this design:

| Component | Location | Purpose |
|-----------|----------|---------|
| `Sidebar.svelte` | Global shell | Fixed 7-item navigation |
| `CommandPalette.svelte` | Global overlay | Ctrl+K search/action |
| `EffectGrid.svelte` | Effects > Browse | Animated thumbnail grid |
| `EffectPreview.svelte` | Effects > Active | Full-width live canvas |
| `ControlPanel.svelte` | Inspector | Auto-generated from metadata |
| `SpatialEditor.svelte` | Layout | Three.js zone placement |
| `DeviceCard.svelte` | Devices | Per-device status + preview |
| `SceneCard.svelte` | Dashboard / Scenes | One-tap scene activation |
| `Toast.svelte` | Global overlay | Non-blocking notifications |
| `SetupWizard.svelte` | First-run | 5-step onboarding flow |
| `DebugPanel.svelte` | Settings > Advanced | Event log, frame timing |
| `AudioSpectrum.svelte` | Inputs / Dashboard | FFT visualizer |
| `Slider.svelte` | Controls | Branded range input |
| `MiniPreview.svelte` | Dashboard | Spatial map with live colors |

## Appendix B: Animation Timing Reference

All animations follow a consistent timing language:

| Duration | Use | Easing |
|----------|-----|--------|
| 100ms | Instant feedback (button press, toggle) | `ease-out` |
| 200ms | UI state changes (hover, focus, nav highlight) | `ease-out` |
| 300ms | Content transitions (panel slide, effect crossfade) | `cubic-bezier(0.4, 0, 0.2, 1)` |
| 500ms | Entrance animations (card appear, wizard step) | `cubic-bezier(0.0, 0, 0.2, 1)` |
| 1000ms+ | Decorative only (logo animation, ambient glow) | `linear` or custom |

All durations halve when `prefers-reduced-motion` is detected. Decorative animations disable entirely.

## Appendix C: Design Token Summary

```css
/* SilkCircuit Neon — Hypercolor Application */
:root {
  /* Brand */
  --hc-purple:        #e135ff;
  --hc-cyan:          #80ffea;
  --hc-coral:         #ff6ac1;
  --hc-yellow:        #f1fa8c;
  --hc-green:         #50fa7b;
  --hc-red:           #ff6363;

  /* Surfaces */
  --hc-bg-deep:       #0a0a0f;
  --hc-bg-base:       #12121a;
  --hc-bg-card:       #1a1a26;
  --hc-bg-elevated:   #222233;
  --hc-bg-float:      #2a2a3d;

  /* Borders */
  --hc-border-subtle: #2d2d44;
  --hc-border-active: rgba(225, 53, 255, 0.2);

  /* Text */
  --hc-text-primary:  #e0e0e8;
  --hc-text-muted:    #8888a0;
  --hc-text-active:   #80ffea;

  /* Typography */
  --hc-font-mono:     "JetBrains Mono", "Fira Code", "SF Mono", monospace;
  --hc-font-sans:     "Inter", -apple-system, "Segoe UI", sans-serif;

  /* Spacing */
  --hc-space-xs:      4px;
  --hc-space-sm:      8px;
  --hc-space-md:      16px;
  --hc-space-lg:      24px;
  --hc-space-xl:      32px;
  --hc-space-2xl:     48px;

  /* Radii */
  --hc-radius-sm:     4px;
  --hc-radius-md:     8px;
  --hc-radius-lg:     12px;
  --hc-radius-xl:     16px;

  /* Shadows (glow-style for dark theme) */
  --hc-glow-purple:   0 0 20px rgba(225, 53, 255, 0.15);
  --hc-glow-cyan:     0 0 20px rgba(128, 255, 234, 0.15);
  --hc-glow-soft:     0 4px 24px rgba(0, 0, 0, 0.4);

  /* Glass */
  --hc-glass-bg:      rgba(10, 10, 15, 0.72);
  --hc-glass-border:  rgba(225, 53, 255, 0.08);
  --hc-glass-blur:    blur(16px) saturate(120%);

  /* Transitions */
  --hc-ease-fast:     100ms ease-out;
  --hc-ease-normal:   200ms ease-out;
  --hc-ease-smooth:   300ms cubic-bezier(0.4, 0, 0.2, 1);
  --hc-ease-enter:    500ms cubic-bezier(0.0, 0, 0.2, 1);
}
```
