# SignalRGB — Display & Effect Ecosystem (Competitive Research)

Researched 2026-06-09 for Hypercolor display faces (hardware LCDs: AIO pump caps,
controller screens). SignalRGB is WhirlwindFX's closed-source, Windows-only RGB
orchestration app. All claims below are dated and sourced.

## TL;DR

SignalRGB shipped **"LCD Faces"** — their name for exactly what Hypercolor calls
display faces — in 2026: beta 2.5.55 (2026-03-25), stable 2.5.66 (2026-06-04).
Faces are plain HTML/JS pages living in the same Lightscript ecosystem as their
RGB effects, dropped into `Documents\WhirlwindFX\LCDFaces`. Launch lineup is
three first-party faces (Now Playing with album art + progress bar, NZXT-styled
dual sensor gauge, Custom Text) plus idle-canvas config, face rotation, and an
audio API (`engine.audio`) on faces. Custom face sideloading is paywalled behind
Pro (~$45/yr). Device coverage is thin: Corsair LCDs (XC7, Capellix, iCUE LINK)
are the supported family; NZXT Kraken LCDs are still explicitly unsupported as
of their own docs, and Lian Li Galahad II LCD support is partial at best. Their
effect system (HTML5 canvas + meta-tag manifest with six control types) is
mature and heavily validated by a large marketplace; their LCD story is brand
new, under-documented, and gated — that's the opening.

## Timeline of Their LCD/Display Investment

| Date | Version | What shipped |
| ---- | ------- | ------------ |
| 2024-05-24 | — | Staff on forum: "we don't offer support for any LCD yet, but this feature is in our roadmap" ([forum](https://forum.signalrgb.com/t/screen-display-issue-corsair-icue-link-h150i-lcd/2318)) |
| 2025-06/07 | 2.5.2/2.5.3 beta, 2.5.6 | Experimental LCD control for Corsair XC7, iCUE LINK, Capellix; circular preview option for plugins ([changelogs](https://docs.signalrgb.com/changelogs/)) |
| 2026-02-03 | 2.5.39 | "LCD Module" enhanced device support/initialization; marketplace remote effects load on demand |
| 2026-03-25 | 2.5.55 beta | LCD Faces debut: Now Playing, NZXT Dual Sensor Gauge, Custom Text; idle canvas, face rotation, album-art zoom/blur/dim, face selection persistence ([changelog](https://docs.signalrgb.com/changelogs/)) |
| 2026-06-04 | 2.5.66 stable | LCD Faces GA; `engine.audio` exposed to faces (spectrum, levels); camera access for effects; Screen Ambience gains blur + motion smoothing; Fan Control and System Monitoring opened to all signed-in users |

Sources: [docs.signalrgb.com/changelogs](https://docs.signalrgb.com/changelogs/) (fetched 2026-06-09).

## 1. Feature Inventory — Displays & Screens

### LCD Faces (the headline feature, 2026)

- **Now Playing** face: song title, artist, album art, track position/duration
  from the **Windows media session**, animated progress bar, with live-applied
  album-art zoom, background blur, and dimming controls.
- **NZXT Dual Sensor Gauge**: "NZXT-styled face showing two hardware sensor
  values simultaneously." Notable: it's an NZXT *CAM-look-alike* rendered on
  the LCDs they do support — aesthetic parity with vendor software, shipped
  even though actual NZXT LCD hardware is unsupported.
- **Custom Text**: configurable text display.
- **Idle Canvas**: configure what the LCD shows when idle.
- **Face rotation** options (circular pump LCDs mount at arbitrary angles).
- Option to disable the "foreground LCD effect layer" — implies a layer model
  where the RGB effect canvas can render behind/around face content.
- **`engine.audio` on faces** (2.5.66): real-time frequency spectrum and levels
  for audio-reactive LCD animations.
- Faces are **HTML files**; community faces install by copying `.html` into
  `%USERPROFILE%\Documents\WhirlwindFX\LCDFaces` and restarting/reselecting
  ([community repo](https://github.com/thetunguskaevent/Guy_Pro_Signal_RGB_LCD_Faces), fetched 2026-06-09).
- **Sideloaded custom LCD faces are a Pro feature** ("Sideloaded custom LCD
  faces for supported devices" — [Pro Features](https://docs.signalrgb.com/guides/account-billing/about-pro-features/), fetched 2026-06-09).

### Device coverage (the weak flank)

- **Corsair**: XC7 waterblock, Capellix-era LCD pump caps, iCUE LINK LCD AIOs —
  experimental since 2.5.6 (2025-07). Their own troubleshooting docs still
  steer users toward iCUE's *hardware GIF* storage ("set up hardware gifs to
  Corsair LCD so you don't need to keep iCue running") and tell you to disable
  the LCD in SignalRGB first ([Corsair doc](https://docs.signalrgb.com/troubleshooting/brand-specific/corsair/), fetched 2026-06-09).
  A 2024 forum thread documents **screen corruption** when the Corsair
  background service is killed while SignalRGB runs ([forum](https://forum.signalrgb.com/t/screen-display-issue-corsair-icue-link-h150i-lcd/2318)).
- **NZXT Kraken (Z series, Elite)**: explicitly unsupported. Docs: "We don't
  support LCD screens from NZXT yet"; they recommend third-party tools
  (Zefir's Flashy Cooler, KrakenZPlayground) ([NZXT doc](https://docs.signalrgb.com/troubleshooting/brand-specific/nzxt/), fetched 2026-06-09).
  Users have been requesting this since at least 2022 ([Z73 thread](https://forum.signalrgb.com/t/nzxt-kraken-z73-lcd-support/4101)) through
  Sept 2025 ([Kraken Elite thread](https://forum.signalrgb.com/t/support-and-or-workaround-to-allow-rgb-control-of-nzxt-kraken-elite-with-lcd-screen/8720)).
  The docs page contradicting the 2026 "NZXT Dual Sensor Gauge" face name is
  confusing users — the face is NZXT-*styled*, not NZXT-hardware.
- **Lian Li Galahad II LCD**: partial/awkward — a 2026 forum thread complains
  the LCD can't even be disabled from SignalRGB ([forum](https://forum.signalrgb.com/t/cant-disable-galahad-ii-lcd-screen-in-signal-rgb/8586));
  Uni Fan TL LCD support was a long-running request ([forum](https://forum.signalrgb.com/t/lian-li-uni-fan-tl-and-tl-lcd/2761)).
- **Generic USB screens (Turing/Turzx-style)**: served by community plugins +
  community HTML faces, not first-party ([Guy_Pro faces repo](https://github.com/thetunguskaevent/Guy_Pro_Signal_RGB_LCD_Faces)).
- **Asus AniMe Matrix / Crosshair OLED**: no support; forum requests only
  ([2023 thread](https://forum.signalrgb.com/t/asus-anime-matrix-support/356),
  [Crosshair OLED thread](https://forum.signalrgb.com/t/does-signalrgb-support-the-asus-crosshair-extreme-oled-screen-and-dot-matrix-screen/8736)).

### Screen Ambience (monitor → RGB mirroring)

Samples display output and maps edge colors onto positioned devices. Free tier
gets a basic version; Pro version claims tighter sampling/lower latency. Gained
blur + motion smoothing in 2.5.66; vertical-monitor crash fixed in 2.5.54.
Costs "roughly 2-3% GPU" per third-party testing
([aurasync free-vs-pro, 2026-04-18](https://aurasync.net/signalrgb-free-vs-pro/)).
Note: this is monitor-to-LED ambience, not device-LCD mirroring.

### Media/video on devices

- "Video lightscripts on third-party devices" is a listed **Pro** feature
  ([Pro Features](https://docs.signalrgb.com/guides/account-billing/about-pro-features/)).
- GIF-on-LCD is delegated to iCUE hardware storage for Corsair (above).
- Camera access for effects landed in 2.5.66.
- "Miniplayer" full-screen canvas viewer (Pro) renders the effect canvas on the
  desktop monitor.

## 2. Effect System & Controls Schema

Effects ("Lightscripts") are literally webpages: HTML + vanilla JS drawing on a
2D `<canvas>`, conventionally **320×200**, driven by
`window.requestAnimationFrame` ([Effects Are Webpages](https://docs.signalrgb.com/developer/lightscripts/it-s-a-webpage/),
[HTML5+JS](https://docs.signalrgb.com/developer/lightscripts/html5-js/), fetched 2026-06-09).
One effect renders once and every device samples it. Three categories: RGB
effects, audio visualizers, game integrations
([Developer Overview](https://docs.signalrgb.com/developer/)).

### Manifest = `<meta>` tags in `<head>`

`<title>` is the effect name; `description` and `publisher` metas carry
attribution; each user control is a meta with a `property` binding to a JS
global:

```html
<meta property="speed" label="Cycle Speed" type="number" min="1" max="10" default="2">
```

### Control types (six total)

From [User Controls](https://docs.signalrgb.com/developer/plugins/user-controls/) (fetched 2026-06-09):

| Type | Attributes | Notes |
| ---- | ---------- | ----- |
| `number` | `property,label,min,max,step,default` | Slider; docs recommend 0–100 ranges |
| `boolean` | `property,label,default` (0/1) | Toggle |
| `hue` | `property,label,min(0-359),max(1-360),default` | Hue-only slider |
| `color` | `property,label,default(#RRGGBB),min,max` | Color picker; full gradient palettes in some effects |
| `combobox` | `property,label,values[],default` | Enum dropdown |
| `textfield` | `property,label,default,filter(regex)` | Free text with optional regex filter |

Typical marketplace effects expose roughly 3–6 controls (speed, scale, colors,
direction, brightness) ([How to Customize Effects](https://docs.signalrgb.com/guides/effects-customization/how-to-customize-effects/)).
Users get a live-preview Customize page, named presets, preset-to-layout
binding, and shareable preset links/export files.

### Runtime APIs exposed to effects

- `engine.audio.freq` — 200-element frequency array
- `engine.audio.level` — overall loudness, −100..0
- `engine.audio.density` — tone roughness, 0..1
  ([Audio Visualizer docs](https://docs.signalrgb.com/developer/lightscripts/audio-visualizer/))
- Camera access (2.5.66+)
- Game integrations feed events via UI analysis or HTTP requests
- Sensor reactivity exists in community tooling: the RGBJunkie visual effect
  builder advertises objects reacting "to real-time hardware data like CPU
  load or temperature" via a Sensor tab ([rgbjunkie.com](https://rgbjunkie.com/)),
  implying an engine-side sensor API, but it is **not documented** in the
  public developer docs as of 2026-06-09.

There are no public developer docs for authoring LCD Faces specifically — the
community reverse-engineers the face folder format. Visual no-code builders
(RGBJunkie, [SRGBmods Effect Creator](https://srgbmods.net/effectcreator/),
[SRGB Interactive Effect Builder](https://joseamirandavelez.github.io/EffectBuilder/))
fill the gap for effects.

## 3. Layout / Positioning Model

Two layers ([About Layouts](https://docs.signalrgb.com/guides/device-configuration/about-layouts/),
[Mapping LED Positions](https://docs.signalrgb.com/developer/plugins/tutorial/mapping-led-positions/), fetched 2026-06-09):

1. **Per-device LED maps** (plugin author's job): parallel arrays
   `vLedNames` / `vLedPositions` of `[x,y]` integer grid coordinates on a
   declared device grid (size = furthest position + 1 in each axis). A paint
   tool in the plugin engine helps map physical LEDs. Circular preview option
   exists for round devices (added 2.5.6).
2. **User layout canvas**: drag each device box on a virtual 2D desk/case
   canvas with exact X/Y coordinates, **scale** (how much of the effect canvas
   the device samples), and **rotation** (direction effects enter/exit). With
   a layout, one wave sweeps the whole rig; without one, devices each run the
   effect independently. Docs suggest the Side-to-Side effect for alignment
   verification. Full layout editor is Pro.

This is canvas-sampling, same family as Hypercolor's SpatialEngine, but grid
coordinates are integer per-device matrices rather than normalized floats, and
the layout is desk-2D only (no depth/zones).

## 4. Marketplace, Pricing, Premium Surface

- **Pricing** (2026): $4.99/mo or ~$35.88–45/yr depending on promo;
  third-party reviews cite $45/yr (2026-04-18, [aurasync](https://aurasync.net/signalrgb-free-vs-pro/));
  $35.88/yr appears in the [OpenRGB comparison](https://aurasync.net/openrgb-vs-signalrgb/).
  Official: [signalrgb.com/pricing](https://signalrgb.com/pricing/).
- **Free**: device support is identical to Pro (key fact — they never gate
  hardware), ~30+ effects, basic screen ambience, 2 macros, lighting profiles.
- **Pro adds**: premium effects (~40–50 first-party), game integrations,
  unlimited macros, fan control + per-device color calibration, video
  lightscripts, Miniplayer, preview update channel, ad removal, **custom LCD
  face sideloading**, full layout editor.
  Note: Fan Control and System Monitoring moved from Pro to all signed-in
  users in 2.5.66 (2026-06-04) — they're loosening the gate over time.
- **Marketplace** ([marketplace.signalrgb.com](https://marketplace.signalrgb.com/)):
  third-party reviews describe "thousands of premium lighting presets" from
  official + community creators ([music-sync-lights.com, Q1 2026](https://music-sync-lights.com/compare/signalrgb/));
  effects load remotely on-demand in playlists since 2.5.39. Free tier shows
  ads in-app.
- Community preset sharing via generated links/export files is free and drives
  Discord/Reddit circulation.

## 5. Sensor / Telemetry Integration

- **System Monitoring** (in-app, free for signed-in users since 2.5.66):
  gauges + sparklines (1 min–24 h history) over a wide sensor set — CPU
  temp/package/core temps, GPU voltage/temp/clocks/load/VRAM, RAM usage and
  frequency, storage temp + IO speeds, fan RPM, **pump RPM + liquid temp +
  temp probes**, network ping/up/down ([System Monitoring](https://docs.signalrgb.com/guides/advanced-features/system-monitoring/), fetched 2026-06-09).
  USB temperature-sensor support added in 2025. RTX 5000 temp readings in 2.5.6.
- **Sensors on LCDs**: only via the NZXT Dual Sensor Gauge face (two values).
  Users want more: "PC Monitoring - More Large Gauge Options"
  ([forum](https://forum.signalrgb.com/t/pc-monitoring-more-large-guage-options/1695)),
  temp-driven effects ([forum](https://forum.signalrgb.com/t/effects-driven-by-sensor-values/1575),
  [forum](https://forum.signalrgb.com/t/temp-monitoring-effect/2553)).
- No documented first-party sensor API for lightscripts; community builders
  expose sensor-reactive objects (see §2).

## 6. Community Sentiment

**Praise** (third-party reviews + forums, 2026):

- The layout editor is the single most-praised feature — "alone changed how
  [the] entire rig looked" ([aurasync free-vs-pro, 2026-04-18](https://aurasync.net/signalrgb-free-vs-pro/)).
- "Pixel-accurate screen ambience" (Pro) rated above competitors; reliable
  multi-brand auto-detection ([aurasync OpenRGB-vs, 2026-04-18](https://aurasync.net/openrgb-vs-signalrgb/)).
- Free tier considered genuinely usable; device support never paywalled.

**Complaints**:

- Subscription resentment: recurring cost on hardware you own
  ([forum pricing thread](https://forum.signalrgb.com/t/pricing-subscription-only/9418)).
- Account requirement + telemetry/entitlement checks; ads in free tier.
- **Forced updates that remove features**: 2.5.66 auto-updated users, removed
  visible custom fan curves; staff confirmed no downgrade path
  (2026-06-06, [forum](https://forum.signalrgb.com/t/forced-update-to-beta-version-2-5-66/9738)).
- Update instability: broken device detection, stuck lighting, settings loss
  ([aurasync](https://aurasync.net/openrgb-vs-signalrgb/)).
- 2–5% idle CPU; screen sync noticeably heavier on low-end systems.
- Windows-only; conflicts with vendor software (Armoury Crate flicker,
  Corsair LCD corruption when iCUE service is killed).
- Years-old unanswered LCD device requests (Kraken Z 2022 → Kraken Elite 2025
  still open); stale docs that contradict shipped features.

## 7. What's Worth Stealing

1. **The face-as-HTML-page model is validated.** Faces live in the same
   HTML/JS ecosystem as effects, installed as single files. Hypercolor already
   renders HTML effects via Servo — faces should be the same pipeline, same
   SDK, same controls schema. Zero new authoring concepts.
2. **Now Playing is the hero face.** Album art + track info + animated
   progress bar, with art zoom/blur/dim knobs. On Linux this maps to MPRIS —
   and SignalRGB physically cannot follow us there (Windows-only). Easy,
   visible differentiation.
3. **Idle canvas + face rotation** are small features users immediately need:
   what shows when nothing is playing, and pump caps mounted at 90°/180°.
   Ship both at face-launch, not later.
4. **Layered composition**: their "foreground LCD effect layer" toggle implies
   RGB-effect-behind-face-content. Hypercolor's compositor already latches
   per-producer surfaces — a face is just another producer layered over the
   zone's effect output.
5. **Vendor-aesthetic parity faces.** They shipped an NZXT-CAM-style gauge for
   people leaving CAM. Faces that look like iCUE/CAM/L-Connect defaults lower
   the switching cost from vendor software.
6. **Sensor gauge faces with real depth**: their dual-sensor gauge is minimal
   and users immediately asked for more layouts. A face SDK with first-class
   sensor bindings (temps, loads, pump/liquid, network) out-runs them on day
   one — they don't even document a sensor API.
7. **Audio on faces** (`engine.audio`-equivalent): spectrum/level/density on
   the face canvas. We already pipe AudioData into FrameInput; expose it to
   faces identically to effects.
8. **Drop-in community folder** for faces, like their `LCDFaces` directory —
   plus the marketplace/preset-link sharing loop that made their effect
   ecosystem big.
9. **Hardware-storage fallback.** Their docs' best Corsair answer is "store a
   GIF to the device's onboard memory so software needn't run." Supporting
   hardware-stored content where protocols allow is a real reliability win.
10. **Meta-manifest simplicity**: six control types cover the whole
    marketplace. Resist schema sprawl; `number/boolean/hue/color/combobox/
    textfield` plus good defaults is demonstrably enough.

## 8. What to Avoid

1. **Don't paywall face sideloading.** Their most creative users (the face
   authors) are the ones charged $45/yr to load their own HTML. Open-source
   Hypercolor wins this community by default — keep it frictionless.
2. **Don't ship the feature before the hardware.** Four years of Kraken LCD
   requests with "not yet" answers poisoned goodwill; the NZXT-styled face on
   non-NZXT hardware reads as trolling to stranded users. Land Kraken/Corsair/
   Lian Li LCD device support in lockstep with the faces feature.
3. **No forced updates, no entitlement phone-home, no account walls.** The
   freshest sentiment data point (2026-06-06) is a user furious about an
   un-consented update that removed fan-curve visibility with no rollback.
4. **Don't leave faces undocumented.** Their community reverse-engineers the
   `LCDFaces` folder; docs cover effects only and the NZXT page contradicts
   the changelog. Ship face authoring docs with the feature, keep them synced.
5. **Don't corrupt the screen on handoff.** The Corsair LCD garbling when
   vendor software is removed is a transport-ownership bug. Own the device
   init/teardown handshake completely or don't claim the device.
6. **Don't gate spatial layout.** Their most-loved feature (layout editor) is
   Pro-gated; reviews resent it. Hypercolor's layouts stay free by nature —
   make that loudly visible in comparisons.
7. **Avoid Windows-session coupling** in face data sources. Their Now Playing
   binds to Windows media sessions only; abstract media/sensor sources behind
   traits (MPRIS, PipeWire, hwmon) so faces are portable.

## Source Index

- https://docs.signalrgb.com/changelogs/ — LCD timeline (fetched 2026-06-09)
- https://docs.signalrgb.com/guides/account-billing/about-pro-features/ — Pro feature list incl. LCD face sideloading
- https://docs.signalrgb.com/developer/ , /developer/lightscripts/it-s-a-webpage/ , /developer/lightscripts/html5-js/ , /developer/lightscripts/audio-visualizer/ — Lightscript architecture + engine.audio
- https://docs.signalrgb.com/developer/plugins/user-controls/ — six control types schema
- https://docs.signalrgb.com/developer/plugins/tutorial/mapping-led-positions/ — vLedNames/vLedPositions grid model
- https://docs.signalrgb.com/guides/device-configuration/about-layouts/ — user layout canvas (x/y/scale/rotation)
- https://docs.signalrgb.com/guides/effects-customization/how-to-customize-effects/ — customize page, presets, sharing
- https://docs.signalrgb.com/guides/advanced-features/system-monitoring/ — sensor inventory
- https://docs.signalrgb.com/troubleshooting/brand-specific/corsair/ , /nzxt/ — LCD support status per brand
- https://forum.signalrgb.com/t/screen-display-issue-corsair-icue-link-h150i-lcd/2318 — 2024 roadmap quote + corruption bug
- https://forum.signalrgb.com/t/support-and-or-workaround-to-allow-rgb-control-of-nzxt-kraken-elite-with-lcd-screen/8720 — Kraken Elite requests (Sept 2025)
- https://forum.signalrgb.com/t/forced-update-to-beta-version-2-5-66/9738 — forced-update sentiment (2026-06-06)
- https://forum.signalrgb.com/t/cant-disable-galahad-ii-lcd-screen-in-signal-rgb/8586 — Lian Li LCD friction
- https://github.com/thetunguskaevent/Guy_Pro_Signal_RGB_LCD_Faces — community face format + install path
- https://aurasync.net/signalrgb-free-vs-pro/ (2026-04-18), https://aurasync.net/openrgb-vs-signalrgb/ (2026-04-18) — pricing, sentiment
- https://music-sync-lights.com/compare/signalrgb/ (Q1 2026) — marketplace scale, pros/cons
- https://rgbjunkie.com/ , https://srgbmods.net/ — community effect builders, sensor reactivity
- https://signalrgb.com/pricing/ , https://signalrgb.com/devices/ , https://marketplace.signalrgb.com/ — official surfaces
