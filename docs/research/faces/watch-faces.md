# Smartwatch Face Systems as Prior Art for Hypercolor Display Faces

Research snapshot: 2026-06-09. Design prior-art survey of smartwatch face
platforms, gathered to inform configurable "display faces" for small hardware
LCDs of wildly different shapes (480x480 round AIO pump caps, 960x160
ultra-wide Push 2 strip, square OLEDs).

## TL;DR

The smartwatch industry spent a decade converging on one architecture, and it
maps almost one-to-one onto what an RGB engine needs for hardware LCD faces:

- **Declarative beats code.** Google deprecated all code-based Wear OS watch
  faces in favor of the XML-based Watch Face Format (WFF). Legacy faces became
  uninstallable from Google Play on January 14, 2026. The renderer lives in
  the platform; faces are pure data (XML + image/font resources). Rationale:
  battery, security (no executable code), zero-maintenance perf improvements,
  and a built-in editor that works for every face automatically.
- **Slots + complications are the universal composition model.** Every
  platform (Apple, Google, Garmin) composes faces from fixed designer-defined
  slots that accept typed data widgets. The data provider declares *what*
  (type + values); the face declares *how to render each type it supports*.
- **A fixed virtual coordinate space + clip shape solves shape adaptivity.**
  WFF faces declare their own width/height and a `clipShape`
  (CIRCLE/RECTANGLE/NONE); the platform scales to the physical panel. Garmin
  ships per-shape resource overlays (`resources-round-218x218`). Nobody does
  fully fluid reflow — they do per-shape layout variants over shared assets.
- **Power-aware rendering is a first-class mode, not an afterthought.** Every
  platform has a two-mode contract (interactive vs ambient/always-on) with
  declarative per-element variants, pixel-illumination budgets (15% on Wear
  OS, 10% on Garmin AMOLED), and per-data-source re-evaluation frequencies.
- **Marketplaces thrive on theming hooks, not raw canvases.** Facer (3M+
  users, 70k faces, 20k designers) and the new Wear OS Watch Face Push API
  (2025) show the winning shape: faces as sandboxed declarative packages,
  pushed from a phone/host app, monetized via subscription + premium tiers.

For Hypercolor: a declarative face format rendered by the engine, with typed
data-source slots, expression bindings with declared update cadences, color
themes as first-class config, and per-shape layout variants is the
battle-tested architecture. Three independent ecosystems landed on it.

---

## Per-Platform Architecture Summaries

### Google Watch Face Format (WFF) — Wear OS

The most directly relevant prior art. WFF is a declarative XML format,
co-developed with Samsung, introduced at I/O 2023 for Wear OS 4
([intro blog, May 2023](https://android-developers.googleblog.com/2023/05/introducing-watch-face-format-for-wear-os.html)).

**Architecture** ([WFF overview](https://developer.android.com/training/wearables/wff)):

- A face is an APK/AAB containing **only resources and a `watchface.xml`** —
  no executable code. The Wear OS system ships a renderer that parses the XML
  and draws the face, pulling in images and fonts as needed.
- Root element `<WatchFace width=.. height=.. clipShape="CIRCLE|RECTANGLE|NONE"
  cornerRadiusX=.. cornerRadiusY=..>` containing one `<Scene>`, plus optional
  `<BitmapFonts>`, `<Metadata>`, `<UserConfigurations>`
  ([WatchFace reference](https://developer.android.com/reference/wear-os/wff/watch-face)).
- The declared width/height define a **virtual coordinate canvas**; all child
  geometry is relative to it, and the platform scales to the physical
  resolution. The face's coordinate space is decoupled from the panel.
- Versioned capability tiers locked to OS releases: WFF v1 (Wear OS 4) → v2
  (weather data, Wear OS 5) → v3 (Wear OS 5.1) → v4 (Wear OS 6, 2025).

**Why Google abandoned code-based faces** (stated rationale across the
[2023 intro](https://android-developers.googleblog.com/2023/05/introducing-watch-face-format-for-wear-os.html)
and the [June 2025 deprecation post](https://android-developers.googleblog.com/2025/06/upcoming-changes-to-wear-os-watch-faces.html)):

1. **Battery/perf**: the platform owns the render loop, so optimizations land
   for every face without an update.
2. **Maintenance**: faces "require less maintenance and fewer updates" — no
   rebuilds for renderer bug fixes.
3. **Security/sandboxing**: "no executable code involved... no code embedded
   in your watch face APK."
4. **Editor for free**: Wear OS ships a system watch face editor that reads
   `UserConfigurations` and generates the customization UI for every face.
5. **Tooling**: non-programmers build faces in Watch Face Studio (Samsung) or
   the Watch Face Designer Figma plugin.

**Deprecation timeline** ([9to5Google, June 12, 2025](https://9to5google.com/2025/06/12/wear-os-legacy-watch-face/)):
after **January 14, 2026**, legacy (Jetpack/Wearable Support Library) faces
can no longer be installed from Google Play, can't be updated, and can't sell
IAP/subscriptions. Already-installed faces keep working. WFF adoption grew
180% year-over-year into May 2025
([What's new in Watch Faces, May 2025](https://android-developers.googleblog.com/2025/05/whats-new-in-watch-faces.html)).

**WFF v4 (Wear OS 6, 2025)** ([May 2025 blog](https://android-developers.googleblog.com/2025/05/whats-new-in-watch-faces.html)):

- **Photos element** — user-selected photos/galleries via companion app.
- **Ambient transitions** — declarative enter/exit animations (e.g.
  `OVERSHOOT` easing with duration control).
- **Color transforms** — `extractColorFromWeightedColors()`, `colorArgb()`;
  dynamic color mapping from weather/complication data.
- **`Reference` element** — define a computed value once, reuse everywhere
  (DRY for expressions).
- **Text autosizing** — `isAutoSize` scales text to fit variable-length data.

### Apple Watch (watchOS)

Apple's position is the inverse of Google's: **no third-party watch faces at
all** — first-party faces only, extensible exclusively through complications
and photo faces. Third-party "face apps" (Facer, Clockology, WatchMaker on
iOS) ship wallpaper + complication bundles, not real faces
([iMore](https://www.imore.com/best-third-party-apple-watch-complications),
[community discussion](https://discussions.apple.com/thread/256081317)).

- **Complications since watchOS 9 are WidgetKit accessory widgets** —
  SwiftUI views driven by a `TimelineProvider` that delivers dated entries
  the system renders ahead of time
  ([Apple docs](https://developer.apple.com/documentation/widgetkit/creating-accessory-widgets-and-watch-complications)).
  This replaced the older ClockKit template system (deprecated, WWDC22's
  "Complications and widgets: Reloaded").
- **Face Gallery UX**: the iPhone Watch app's gallery is the discovery
  surface; watchOS 26 reorganized it into curated categories (Health &
  Fitness, Photos, Colorful, Clean, Data Rich, Tool, Bold...)
  ([Apple newsroom, June 2025](https://www.apple.com/newsroom/2025/06/watchos-26-delivers-more-personalized-ways-to-stay-active-and-connected/)).
  Live preview updates as you tweak color/style/complication options.
- **Face sharing**: `.watchface` files capture a full configuration — base
  face, colors, styles, complication choices — shareable via Messages/
  AirDrop/files since watchOS 7
  ([Apple support](https://support.apple.com/guide/watch/share-apple-watch-faces-apdb3107c16a/watchos)).
  A face is a *parameterization of a template*, not an arbitrary artwork —
  this is what makes sharing safe and tiny.
- **Smart Stack**: a scrollable stack of widgets below the face, ranked by a
  relevance model. watchOS 26 (released September 15, 2025) added
  customizable widget content and "hints" — proactive suggestions rendered
  in Liquid Glass at the bottom of the face
  ([9to5Mac, Dec 30, 2025](https://9to5mac.com/2025/12/30/watchos-26-has-three-big-new-updates-for-apple-watch-faces/)).
- **watchOS 26 faces**: four new faces (Flow with Liquid Glass numerals,
  Exactograph regulator, Waypoint compass for Ultra, Hermès Faubourg);
  ticking-seconds always-on expanded to more faces on Ultra 3 / Series 11.

### Garmin Connect IQ

The only major platform still running **developer code on-device**: Monkey C
compiled apps, with four app types (watch faces, data fields, widgets/glances,
device apps).

- **Data fields model**: third-party code renders inside a slot of an
  activity screen layout. *Simple* data fields supply label + value into a
  system-rendered slot; *complex* data fields get a draw context for the
  slot region ([Garmin UX guidelines](https://developer.garmin.com/connect-iq/user-experience-guidelines/data-fields/)).
  The user picks which field occupies which slot of which layout — the
  closest analog to "user assigns a data source to a region."
- **Per-shape resources**: the SDK's `devices.xml` defines device families by
  shape and resolution — `round-218x218`, `semiround-215x180`,
  `rectangle-148x205`, etc. Resource overlay folders
  (`resources-round`, `resources-rectangle-205x148`) supply per-family or
  per-device layouts over a shared base; most-specific qualifier wins
  ([Garmin layouts](https://developer.garmin.com/connect-iq/core-topics/layouts/),
  [resources](https://developer.garmin.com/connect-iq/core-topics/resources/)).
  In practice many face devs skip XML layouts and branch on
  width/height/obscurity flags in code
  ([Garmin forums](https://forums.garmin.com/developer/connect-iq/f/discussion/2768/datafields-best-practices-for-using-layouts-should-i-use-layouts-at-all)).
- **AMOLED always-on rules**: in low-power mode a face updates **once per
  minute** and each frame may illuminate at most **10% of pixels**; firmware
  adds pixel-shifting and strategic dimming for burn-in
  ([Garmin FAQ](https://developer.garmin.com/connect-iq/connect-iq-faq/how-do-i-make-a-watch-face-for-amoled-products/)).
- **Monetization (2024–2025)**: Connect IQ Store added direct paid apps via
  Garmin Pay — $100/yr merchant fee, Garmin takes 15% of tax-exclusive price
  ([Garmin monetization](https://developer.garmin.com/connect-iq/monetization/),
  [press release](https://www.garmin.com/en-US/newsroom/press-release/wearables-health/garmin-enables-premium-app-purchases-in-the-connect-iq-store-and-unveils-fun-new-watch-faces-and-apps/)).
  Before that, devs bolted on third-party "unlock code" systems — a long
  community pain point.

### Facer / WatchMaker / Pujie / KWCH — community ecosystems

- **Facer** is the largest cross-platform face marketplace: 3M+ users, 70k+
  faces, 20k designers. Monetization: Facer Premium subscription
  ($19.99 first year, $39.99/yr regular) for all premium faces + features
  (color customization, interactive widgets); à la carte purchases; and an
  invite-only **Creator Partner Program** for revenue share
  ([Facer 5.0 announcement](https://news.facer.io/introducing-facer-5-0-and-facer-premium-3edd3b4a1d2c),
  [partner page](https://www.facer.io/creator/partner)).
  What makes it thrive: zero-friction browsing on the phone with one-tap
  push to the watch, a web-based creator tool with live data preview, brand
  partnerships, and a free tier large enough to seed network effects.
- **WatchMaker**: face builder with **Lua scripting** for behavior
  ([WatchMaker wiki](https://watchmaker.haz.wiki/lua)) — the
  power-user end of the spectrum, and the cautionary tale: arbitrary script
  inside faces is exactly what WFF eliminated.
- **Pujie (Black)**: on-phone designer for Wear OS with deep parametric
  control; library sharing of both faces and components
  ([pujie.io](https://pujie.io/)).
- **KWCH** (Kustom Watchface Creator, from the KWGT widget-maker devs,
  launched Sept 2023): WYSIWYG editor, formula language for data binding,
  gradients/shadows/3D transforms, free + paid pro
  ([Android Authority](https://www.androidauthority.com/kwch-wear-os-custom-watch-face-maker-3368770/),
  [9to5Google](https://9to5google.com/2023/09/26/wear-os-custom-watch-faces-kwch-app/)).
  Kustom's "formulas everywhere" model — any property can be an expression —
  is the strongest community-tool pattern.
- All of these were forced onto WFF by the 2026 legacy cutoff; Facer,
  TIMEFLIK, WatchMaker, Pujie, and Recreative became launch partners for the
  Watch Face Push API marketplace model
  ([Android Central](https://www.androidcentral.com/apps-software/wear-os/wear-os-6-will-bring-facer-back-onto-android-watches)).

### Pebble (bonus, 2025 relaunch)

PebbleOS went 100% open source and Core Devices shipped Pebble 2 Duo and
Pebble Time 2 in late 2025, with the appstore relaunched on Rebble's backend
(2,000+ apps, 10,000+ faces)
([The Register, Nov 25, 2025](https://www.theregister.com/2025/11/25/pebble_eink_smartwatch_open_source/),
[appstore relaunch](https://ericmigi.com/blog/re-introducing-the-pebble-appstore/)).
Relevant detail: old faces render **letterboxed with a border** on the
larger Pebble Time 2 screen until updated — the fallback strategy when a
fixed-resolution face meets a new panel size.

---

## The Complication / Slot Model in Depth

The single most transferable architecture. All three major platforms agree on
the same separation of concerns:

> **Data sources declare typed payloads. Faces declare slots, the types each
> slot accepts, and how to render each accepted type. The user binds sources
> to slots in an editor.**

### WFF's version ([complication guide](https://developer.android.com/training/wearables/wff/complications), [ComplicationSlot reference](https://developer.android.com/reference/wear-os/wff/complication/complication-slot))

- `<ComplicationSlot slotId=.. supportedTypes="SHORT_TEXT SMALL_IMAGE EMPTY"
  x=.. y=.. width=.. height=..>` — max **8 slots per face**; each slot has a
  localized display name for the editor and a flag controlling whether the
  user may change the bound provider.
- **Typed payload vocabulary**: `SHORT_TEXT`, `LONG_TEXT`, `RANGED_VALUE`,
  `GOAL_PROGRESS`, `MONOCHROMATIC_IMAGE`, `SMALL_IMAGE`, `PHOTO_IMAGE`,
  `WEIGHTED_ELEMENTS`, `EMPTY`.
- **Per-type render branches**: inside the slot, one `<Complication
  type="SHORT_TEXT">` block per supported type, built from ordinary WFF
  primitives (`PartText`, `PartImage`, `PartDraw`) binding expressions like
  `[COMPLICATION.TEXT]`, `[COMPLICATION.VALUE]`,
  `[COMPLICATION.TARGET_VALUE]`, `[COMPLICATION.SMALL_IMAGE]` (with an
  `_AMBIENT` variant for low-power art).
- **Bounding shapes**: `BoundingOval` vs `BoundingBox` (plus edge arcs)
  define the live region — the same slot can be a circle on a round face.
- **`DefaultProviderPolicy`** picks the out-of-box source per slot (e.g.
  `defaultSystemProvider="STEP_COUNT"` with a fallback type), so a face looks
  alive before the user configures anything.
- Best practice: support multiple types per slot for maximum provider
  compatibility; degrade gracefully when optional payload fields (title,
  icon) are absent.

### Apple's version ([WidgetKit accessory widgets](https://developer.apple.com/documentation/widgetkit/creating-accessory-widgets-and-watch-complications))

- **Families = shape classes**, not pixel sizes: `accessoryCircular`,
  `accessoryRectangular`, `accessoryInline` (text-only line),
  `accessoryCorner` (watch-only; small content + curved gauge/label).
  A face's slots each accept specific families; faces have 1–5 slots.
- **Timeline model**: providers return dated entries ahead of time
  (`placeholder` / `getSnapshot` / `getTimeline` + refresh policy); the
  system pre-renders entries so the face never blocks on the data source.
- **Rendering modes**: every complication must render in `fullColor`,
  `accented` (system tints a flattened hierarchy), and `vibrant`
  (desaturated monochrome for always-on). `widgetLabel` and
  `AccessoryWidgetBackground` adapt content to the slot's context.
- The pre-watchOS-9 ClockKit system was template-based (fixed layouts per
  family that you filled with text/image/gauge providers) — even more
  declarative, and a good reminder that **templates per slot-shape** are
  enough for 90% of complications
  ([Atomic Object overview](https://spin.atomicobject.com/watchos-complications-families-templates/)).

### Garmin's version

Data fields are the same idea applied to activity screens: the user picks a
screen layout (1–4+ slots), then assigns a built-in or Connect IQ data field
to each slot. Simple fields hand the system a label/value pair; complex
fields own the slot's draw call.

### Hypercolor mapping

A Hypercolor display face = `<Face>` with slots accepting typed payloads
(`temperature`, `ranged_value` for fan/pump %, `short_text`, `graph_series`,
`album_art`, `audio_spectrum`...). Providers: HWMon sensors, now-playing,
audio analysis, scene/effect state, network drivers. `DefaultProviderPolicy`
≈ sensible default bindings per face. The editor enumerates slots by display
name exactly like the Wear OS system editor does.

---

## Shape-Adaptivity Techniques

Observed techniques, strongest first:

1. **Virtual canvas + declared clip shape (WFF).** The face picks its own
   coordinate space (`width`/`height`) and declares `clipShape`
   CIRCLE / RECTANGLE (+ corner radii) / NONE. The renderer scales
   proportionally to the physical panel. Faces are resolution-independent by
   construction — the same philosophy as Hypercolor's normalized [0,1]
   spatial coordinates.
2. **Shape/size family overlays (Garmin).** One codebase, per-family resource
   directories (`resources-round-218x218`, `resources-semiround`,
   `resources-rectangle-148x205`); most-specific wins. Per-shape *layout
   variants* over shared assets, not fluid reflow.
3. **Slot families as shape classes (Apple).** Complication content targets
   abstract shapes (circular, rectangular, inline, corner) rather than
   coordinates; the face places those shapes. Content authored once renders
   in any slot of that family.
4. **Edge-aware primitives.** WFF bounding **arcs** hug a round bezel;
   Apple's `accessoryCorner` curves a gauge around a corner; Garmin exposes
   "obscurity flags" telling a data field which of its corners are clipped
   by a round screen. Round displays want polar primitives (arcs, radial
   ticks), not just rectangles.
5. **Letterbox fallback (Pebble).** When a fixed-size face meets a bigger
   panel, render at native size with a border. Ugly but never broken.

**Hypercolor implication:** aspect ratio is the hard case the watch world
never faced — 960x160 (6:1) shares no usable layout with 480x480 round. The
Garmin overlay model fits best: a face declares layout variants per shape
class (`round`, `square`, `wide-strip`), sharing palette, data bindings, and
assets, with the WFF-style virtual canvas inside each variant. A face that
lacks a `wide-strip` variant can letterbox or be filtered from that device's
gallery (Apple-style capability gating).

---

## Customization / Theming Models

### Declarative config schema → auto-generated editor (WFF)

`<UserConfigurations>` declares the entire customization surface
([UserConfigurations reference](https://developer.android.com/reference/wear-os/wff/user-configuration/user-configurations)):

- **`ColorConfiguration`** — 1–100 `ColorOption` swatches (each can be a
  multi-color set). Any color attribute (`tintColor` etc.) binds to the
  selection via expression.
- **`ListConfiguration`** — enumerated style options (tick styles, hand
  shapes, layout densities).
- **`BooleanConfiguration`** — toggles, referenced from `Condition`
  expressions.
- **`PhotosConfiguration`** — user photo sources (WFF v4).
- **`Flavor`** — named presets bundling specific option values: "the face's
  designer-curated colorways." These power preview thumbnails and one-tap
  theming
  ([Flavor reference](https://developer.android.com/reference/wear-os/wff/user-configuration/flavor)).

The system editor (watch + phone) renders all of this with live preview —
face authors never build editor UI. This is the highest-leverage pattern in
the whole survey.

### Parameterized templates (Apple)

Faces expose designer-chosen axes (color, style, density, complication
slots); the user twists the Digital Crown through options with live preview.
A complete configuration serializes to a tiny shareable `.watchface` file.
Customization-as-data enables sharing, galleries, and server-side rendering
of previews.

### Formulas everywhere (Kustom/WatchMaker)

Every visual property can be an expression over live data. Maximum power,
but WatchMaker's Lua shows the cost: unauditable faces, battery surprises,
and a sandboxing dead end (it's why these apps had to be rebuilt for WFF).
WFF's bounded expression language is the deliberate middle ground.

### Expression / data-binding language (WFF) — the keystone

([Build expressions](https://developer.android.com/training/wearables/wff/expressions)):

- Data sources in square brackets: `[SECOND]`, `[DAY_OF_WEEK]`,
  `[AMPM_STATE]`, `[ACCELEROMETER_ANGLE_X]`, `[STEP_COUNT]`, weather,
  `[CONFIGURATION.optionId]`, `[COMPLICATION.*]`.
- Arithmetic, comparison, logical, ternary operators plus functions like
  `clamp()`; used in three contexts: `Transform` (animate any attribute),
  `Condition` (show/hide), `Template` (string formatting).
- **Re-evaluation is driven by source cadence**: an expression on
  `[DAY_OF_WEEK]` re-evaluates daily; `[SECOND]` every second. Docs push
  authors toward the lowest-frequency source that answers the question
  (`[AMPM_STATE] == 1` over `[SECONDS_IN_DAY] > 43200`). The engine can
  statically know each element's update rate from its bindings — perfect for
  budgeting refresh on slow buses (USB-attached LCDs).

### Power-aware rendering modes

- **Wear OS**: ambient mode targets ≤15% pixels illuminated; per-element
  `<Variant mode="AMBIENT" target="alpha" value="0"/>` swaps appearance
  declaratively; animation guidance caps at 15 fps; WFF v4 adds animated
  ambient enter/exit transitions
  ([ambient guide](https://developer.android.com/training/wearables/wff/ambient)).
- **Apple**: three mandatory rendering modes (fullColor/accented/vibrant);
  the system pre-renders timelines so AOD never wakes the data source.
- **Garmin AMOLED**: low-power mode = 1 update/minute, ≤10% pixels, plus
  firmware pixel-shifting and dimming.

For Hypercolor LCDs the analog is real: OLED pump caps burn in, USB
bandwidth is finite, and an idle rig may want a dimmed clock instead of a
60 fps visualizer. A declarative `ambient`/`active` variant per element plus
per-binding update cadences gives the engine everything it needs to throttle
gracefully without nerfing the active mode.

---

## What Transfers Directly to RGB-Engine Display Faces

1. **Faces as pure data, renderer in the engine.** WFF's core bet, validated
   by a platform-wide forced migration. Hypercolor already renders effects
   from declarative-ish sources; faces should be packages (manifest + layout
   + assets), never plugins with code. Security, hot-reload, marketplace
   safety, and "perf improvements land for every face" all follow.
2. **Typed slot/complication model.** Slots with `supportedTypes`, per-type
   render branches, and default providers map directly onto sensor readouts,
   now-playing, clocks, and meters. Max-8-slots is a sane complexity cap.
3. **Expression bindings with declared cadence.** `[CPU_TEMP]`,
   `[FAN_RPM.0]`, `[AUDIO.BASS]`, `[SCENE.NAME]` in square-bracket
   expressions, with engine-known refresh rates per source — static update
   budgeting per face, per panel.
4. **ColorConfiguration / Flavors.** Hypercolor faces should bind to the
   active scene palette the way WFF faces bind to theme colors — and
   designer-curated Flavors are exactly "face presets" for the gallery. WFF
   v4's weighted-color extraction (derive accents from album art / canvas
   content) is a gorgeous fit for an RGB engine.
5. **Auto-generated editor from config schema.** Declare the customization
   surface in the face package; the web UI renders pickers with live preview
   for every face with zero per-face UI work. Proven by the Wear OS system
   editor and Watch Face Studio.
6. **Virtual canvas + clipShape + shape-class layout variants.** Round 480px
   caps and square OLEDs are literally the watch problem; the strip needs a
   Garmin-style `wide` variant. Capability-gate the gallery per panel.
7. **Ambient/active dual-mode contract.** Per-element variants + pixel/fps
   budgets for idle, burn-in-prone, or bandwidth-constrained panels.
8. **Push-API marketplace shape.** Watch Face Push (Wear OS 6, 2025) is the
   exact topology Hypercolor has: a host app (daemon/web UI) curates and
   pushes validated declarative packages to constrained display targets,
   with validators run at publish time
   ([Watch Face Push](https://developer.android.com/training/wearables/watch-face-push)).
   Google ships an open-source WFF validator + memory-footprint checker
   ([github.com/google/watchface](https://github.com/google/watchface)) —
   precedent for `hypercolor face lint`.
9. **Shareable face files.** Apple's `.watchface` (template + parameter
   values) shows configurations-as-tiny-files enable community sharing
   without shipping assets.

## What Doesn't Transfer

- **Timeline pre-rendering (Apple).** Built for radios that sleep for
  minutes; Hypercolor's data sources are local and hot (sensors at Hz,
  audio at 60 fps). Live binding is strictly better here.
- **Battery as the prime constraint.** Desktop LCDs are powered; the
  budgets that matter are USB/HID bandwidth, panel refresh, and OLED
  burn-in — keep the *mechanism* (modes, budgets) but swap the constants.
- **OS-gatekeeper review and store policy.** Play/App Store review,
  merchant fees, revenue splits — irrelevant to an open-source engine,
  except as marketplace-design folklore.
- **Wrist-specific inputs.** Gyro/tilt expressions, wrist-flick gestures,
  on-face tap targets — mostly meaningless for a pump cap. (Though
  `[ACCELEROMETER_ANGLE]`-driven parallax has a cousin: audio- and
  cursor-reactive parallax.)
- **Square-bracket-only minimal expression language.** WFF's language is
  deliberately tiny because faces run on a wrist SoC. Hypercolor already
  has richer compute paths (native + Servo/HTML effects); the face format
  should stay small like WFF, but the ceiling for "escape hatch" content is
  an HTML effect in a slot, which WFF has no equivalent of.
- **Single-shape monoculture assumptions.** Apple optimizes for exactly one
  display shape per generation; watch platforms never solved extreme aspect
  ratios. The 960x160 strip needs layout thinking none of these platforms
  provide out of the box.

---

## 2025–2026 Developments Timeline

| Date | Event |
| --- | --- |
| May 2025 (I/O) | WFF v4 previewed for Wear OS 6: photos, ambient transitions, color transforms, `Reference`, text autosizing; Watch Face Push API announced; WFF usage +180% YoY ([blog](https://android-developers.googleblog.com/2025/05/whats-new-in-watch-faces.html)) |
| June 2025 | Google announces legacy watch face removal timeline ([blog](https://android-developers.googleblog.com/2025/06/upcoming-changes-to-wear-os-watch-faces.html), [9to5Google](https://9to5google.com/2025/06/12/wear-os-legacy-watch-face/)) |
| June 2025 (WWDC) | watchOS 26 announced: Liquid Glass, redesigned categorized Face Gallery, customizable Smart Stack widgets, hints ([Apple](https://www.apple.com/newsroom/2025/06/watchos-26-delivers-more-personalized-ways-to-stay-active-and-connected/)) |
| Aug 2025 | Watch Face Designer Figma plugin: design → one-click export to Play Store/Android Studio/APK ([blog](https://android-developers.googleblog.com/2025/08/introducing-watch-face-designer.html)); Wear OS Spotlight Week; Facer/TIMEFLIK/WatchMaker/Pujie/Recreative confirmed as Watch Face Push marketplace partners ([Android Central](https://www.androidcentral.com/apps-software/wear-os/wear-os-6-will-bring-facer-back-onto-android-watches)) |
| Sep 15, 2025 | watchOS 26 ships ([9to5Mac](https://9to5mac.com/2025/09/15/watchos-26-is-now-available-heres-whats-new-for-apple-watch/)) |
| Oct–Nov 2025 | Pebble appstore relaunch on Rebble backend; Pebble Time 2 ships; PebbleOS fully open-sourced ([The Register](https://www.theregister.com/2025/11/25/pebble_eink_smartwatch_open_source/)) |
| Dec 2025 | Androidify ships a Watch Face Push generative face pipeline (app-generated faces pushed to watch) ([blog](https://android-developers.googleblog.com/2025/12/bringing-androidify-to-wear-os-with.html)) |
| Jan 14, 2026 | Legacy Wear OS faces become uninstallable/unsellable on Google Play; WFF is the only path forward |

## Source Index

- WFF overview / setup / ambient / expressions / complications / Watch Face Push: developer.android.com/training/wearables/{wff,watch-face-push} (accessed 2026-06-09)
- WFF XML reference (WatchFace, ComplicationSlot, UserConfigurations, ColorConfiguration, Flavor): developer.android.com/reference/wear-os/wff/ (accessed 2026-06-09)
- Android Developers Blog: intro (2023-05), what's new (2025-05), deprecation (2025-06), Watch Face Designer (2025-08), amoledwatchfaces migration (2025-08), Androidify Push (2025-12)
- google/watchface validator tooling: github.com/google/watchface
- Apple: WidgetKit accessory widgets docs, WWDC22 10050, watchOS 26 newsroom (2025-06), 9to5Mac face roundup (2025-12-30), face sharing support docs
- Garmin: connect-iq core-topics (layouts, resources), UX guidelines (watch faces, data fields), AMOLED FAQ, monetization docs + press release (2024), forums on resource qualifiers and AOD rules
- Facer: news.facer.io (5.0/Premium, Wear OS 6 update), facer.io/creator/partner, help.facercreator.io
- Kustom KWCH: Android Authority + 9to5Google launch coverage (2023-09)
- Samsung: Watch Face Studio user guide (tag expressions, always-on), WFF build blog (2025-08-14)
- Pebble: ericmigi.com blog posts (2025), The Register (2025-11-25), 9to5Google (2025-10-13)
