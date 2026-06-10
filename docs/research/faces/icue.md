# Corsair iCUE LCD / Screen Ecosystem — Competitive Analysis

Research date: 2026-06-09. Competitive analysis for Hypercolor display faces.
All facts dated and sourced; prices in USD unless noted.

## TL;DR

Corsair has quietly become the most aggressive player in the "screens on PC
hardware" space, and in 2025-2026 they made two strategic moves that matter
for Hypercolor:

1. **They turned screen content into an open HTML platform.** iCUE widgets
   are HTML/CSS/JS apps running on QtWebEngine (Chromium), packaged as
   `.icuewidget` files with a `manifest.json`, scaffolded by a CLI
   (`icuewidget init/validate/package`), distributed through the Elgato
   Marketplace (launched May 2026, iCUE 5.44), with paid community widgets
   planned. They even ship an **AI skill for coding agents** in the
   WidgetBuilder Kit. This is exactly Hypercolor's HTML-effect architecture,
   pointed at screens instead of LEDs.

2. **They're bifurcating the hardware into "real monitor" and "USB face"
   classes.** New 5" pump screens (720x1280) and the 14.5" Xeneon Edge
   (2560x720) are DisplayPort devices — the OS sees them as monitors, so any
   content works. The legacy 480x480 round pump caps remain USB devices fed
   images/GIFs by iCUE, capped at 30fps, no video, no webcam.

The weaknesses are equally instructive: iCUE is Windows-only for screen
content, locks hardware sensors away from other tools, widget layout is a
constrained slot grid rather than freeform, the 480x480 class can't show
video or live arbitrary content, and Murals (their screen-ambient lighting
sync) does not drive the LCDs at all — lighting and screens are two separate
configuration worlds. Linux users are served only by reverse-engineered
community tools (OpenLinkHub).

## Hardware Inventory

| Device | Screen | Resolution / shape | Transport | Notes |
| --- | --- | --- | --- | --- |
| iCUE H100i/H150i/H170i ELITE LCD (2021) + XT refresh (2023) | 2.1" round IPS pump cap | 480x480, 600 cd/m², 30fps GIF cap | USB 2.0 header | H150i XT ~$289.99. Upgrade kit retrofits ELITE CAPELLIX. ([corsair.com](https://www.corsair.com/us/en/p/cpu-coolers/cw-9060075-ww/icue-h150i-elite-lcd-xt-display-liquid-cpu-cooler)) |
| iCUE LINK H100i/H150i/H170i LCD (2023) | 2.1" IPS pump cap | 480x480, 600 cd/m², 24-bit, 30fps GIF cap | iCUE LINK hub (counts as 1 of 24 hub devices) | ([corsair.com](https://www.corsair.com/us/en/explorer/diy-builder/cpu-coolers/corsair-icue-link-lcd-aio-coolers-and-icue-link-lcd-upgrade-kit-everything-you-need-to-know/)) |
| iCUE LINK LCD Screen Module (2023) | 2.1" IPS snap-on cap, 24 ARGB LEDs behind | 480x480 | iCUE LINK | Tool-free pump-cap swap for LINK AIOs (~$80 street, unconfirmed). ([corsair.com](https://www.corsair.com/us/en/p/cpu-coolers/cw-9061011-ww/icue-link-lcd-screen-module-black-cw-9061011-ww)) |
| iCUE LINK TITAN 240/360 RX LCD (2024) | 2.1" IPS pump cap | 480x480, 600 cd/m², 30fps | iCUE LINK | $220/£260 (360). PC Gamer: only makes sense "inside a brand new iCUE Link system". ([pcgamer.com](https://www.pcgamer.com/hardware/cooling/corsair-icue-link-titan-360-rx-lcd-review/)) |
| XC7/XD5 ELITE LCD water blocks | small LCD | — | iCUE LINK | Listed as `pump_lcd` targets in the widget SDK. ([docs.elgato.com](https://docs.elgato.com/icue/widgets/)) |
| iCUE Nexus (2020) | 5" touch strip companion (keyboard-mount or standalone) | 640x48 touch | USB | Effectively discontinued by 2024-2025; minimal feature set. ([forum.corsair.com](https://forum.corsair.com/forums/topic/189905-icue-nexus-discontinued/)) |
| Xeneon Edge 14.5" LCD Touchscreen (Aug 26, 2025) | 14.5" ~32:9 AHVA, 5-point touch | 2560x720, 60Hz, ~350 nits, 183 PPI | HDMI or USB-C DP alt mode | $249.99. Desk stand, tripod, 360mm-radiator and magnetic mounts. Crystal/Smoke/Atomic Purple colorways added 2026. ([kitguru.net](https://www.kitguru.net/peripherals/mat-mynett/corsair-xeneon-edge-review-14-5-touchscreen/), [pcworld.com](https://www.pcworld.com/article/2888302/corsair-xeneon-edge-14-5-review.html)) |
| FRAME 4000D LCD RS ARGB case (2026) | Xeneon Edge integrated under main chamber | 2560x720 | DP | ~$400 case with built-in touchscreen. ([tomshardware.com](https://www.tomshardware.com/tech-industry/corsair-builds-multi-function-touchscreen-lcd-into-a-usd400-case-frame-4000d-enclosure-gets-a-modular-xeneon-edge-upgrade)) |
| VANGUARD 96 / VANGUARD PRO 96 keyboards (2025) | 1.9" IPS keyboard LCD + rotary dial | 320x170 | USB/wireless | Widgets, profile art, Elgato Virtual Stream Deck pairing; configured via Corsair Web Hub. `keyboard_lcd` SDK target. ([corsair.com](https://www.corsair.com/us/en/explorer/gamer/keyboards/vanguard-pro-96/)) |
| iCUE LINK 5" LCD Screen Module (Computex 2026) | 5" portrait IPS pump cap, 32 RGB LEDs behind, 70mm VRM fan | 720x1280, 60Hz, 500 cd/m², 24-bit | DisplayPort (+ LINK) | Acts as a **secondary Windows monitor**. Tool-free CapSwap onto TITAN (II) RX. ([corsair.com](https://www.corsair.com/us/en/explorer/diy-builder/cpu-coolers/corsair-icue-link-5-inch-lcd-screen-module-everything-you-need-to-know/)) |
| iCUE LINK TITAN II ULTRA 360 LX LCD (Computex 2026) | 5" IPS on pump | 720x1280 | DisplayPort | Same 5" module integrated; dual-layer radiator, FlowDrive II pump. ([corsair.com](https://www.corsair.com/us/en/explorer/diy-builder/cpu-coolers/icue-link-titan-ii-ultra-360-lx-lcd-aio-everything-you-need-to-know/), [rkiologist.com](https://rkiologist.com/2026/06/04/corsair-shows-off-the-icue-link-titan-ii-ultra-360-lx-lcd-at-computex-2026/)) |

## Face / Widget Feature Inventory

### 480x480 pump caps (ELITE LCD, LINK LCD, TITAN RX LCD)

Per Corsair's setup guides ([help.corsair.com](https://help.corsair.com/hc/en-us/articles/4412131516813-ELITE-LCD-How-to-set-up-your-ELITE-LCD-in-iCUE-4-or-newer),
[corsair.com Constructor guide](https://www.corsair.com/uk/en/explorer/diy-builder/cpu-coolers/create-a-custom-lcd-screen-for-your-aio-in-icue/)):

- **Presets**: ready-made sensor readout faces (coolant temp, CPU temp, etc.).
- **Custom image/GIF**: PNG, BMP, JPG, JPEG, GIF up to 480x480; GIFs capped
  at 30fps. Built-in **Giphy search**. Zoom / center / rotate on import.
- **Constructor** (custom face editor): background image/GIF + up to **three
  sensors** overlaid, editable or hideable labels, label above/below value,
  text color control. Alternative: a clock face instead of sensors.
- **Widgets arrived 2025**: pump LCDs got a Clock Widget with multiple faces
  (iCUE 5.36, Nov 2025) and a Calendar Widget (iCUE 5.38, Dec 2025). The
  widget SDK targets them as `pump_lcd`. ([vortez.net](https://www.vortez.net/news_story/corsair_rolls_out_icue_5_36_update_with_new_widgets_and_major_stability_fixes.html), [x.com](https://x.com/CORSAIR/status/2000736212189954456))
- **Hardware screen**: one face (default or a chosen image/GIF) is flashed to
  onboard memory and plays when iCUE isn't running. Everything else requires
  iCUE alive. ([help.corsair.com](https://help.corsair.com/hc/en-us/articles/4420187201293-ELITE-LCD-What-to-do-if-your-ELITE-LCD-Display-only-shows-the-hardware-screen), [forum.signalrgb.com](https://forum.signalrgb.com/t/corsair-h150i-lcd-custom-screen-support/4130))
- **Not supported**: video files, webcam feeds, screen mirroring, browser
  content. Users asking for video are told to convert to GIF.
  ([forum.corsair.com "Videos on LCD Cooler?"](https://forum.corsair.com/forums/topic/174656-videos-on-lcd-cooler/))
- Device settings: brightness, target framerate.

### Xeneon Edge / 5" DP modules (the "real monitor" class)

Because they're DisplayPort monitors, anything Windows can render works.
On top of that, iCUE overlays its widget dashboard:

- **First-party widgets** (20+ free as of May 2026): Sensor Chart, 2 Sensors,
  Sensors list, Windows Notifications, Media, YouTube, Twitch Chat, Twitch
  Live, Volume, Web URL, iFrame, Image/Video, Slideshow, Launch App,
  Stopwatch, SimHub, Clock Face, Weather, Calendar, Air Quality, Decision
  Coin, Chronograph Stopwatch, virtual keyboard (5.38).
  ([corsair.com widgets explained](https://www.corsair.com/us/en/explorer/diy-builder/accessories/xeneon-edge-widgets-explained/))
- **Up to 30 widget pages** per PCWorld's review. ([pcworld.com](https://www.pcworld.com/article/2888302/corsair-xeneon-edge-14-5-review.html))
- **Elgato Virtual Stream Deck** integration turns it into a touch macro pad.
- Monthly widget drops promised from summer 2025; cadence held through 2026
  patch notes (5.36, 5.38, 5.44, 5.46.67 of June 2, 2026 added double-click
  widget install, dark-mode Twitch chat, background transparency slider).
  ([corsair.com 5.46.67 notes](https://www.corsair.com/us/en/explorer/release-notes/icue/icue-54667/))

### The widget SDK (the strategically important part)

([docs.elgato.com/icue/widgets](https://docs.elgato.com/icue/widgets/),
[corsair.com custom widget guide](https://www.corsair.com/us/en/explorer/diy-builder/accessories/how-to-create-a-custom-widget-for-the-xeneon-edge/))

- Widgets are **HTML, CSS, and JavaScript on QtWebEngine** (Chromium).
- Package = `index.html` + `manifest.json` (id in reverse-DNS, version,
  `supported_devices`, `min_framework_version`, required plugins, SVG
  preview icon), zipped as `.icuewidget`.
- Device targets: `dashboard_lcd` (Xeneon Edge), `pump_lcd` (XC7/XD5/AIO
  LCDs), `keyboard_lcd` (Vanguard 96). One widget, multiple screen classes,
  responsive slot sizes from "Small (840x344)" to "Extra Large (2536x696)"
  on the Edge.
- Runtime: `icueEvents` object with `onICUEInitialized` and `onDataUpdated`
  callbacks; user-configurable properties declared via `x-icue-property`
  meta tags (`data-type`, `data-label`, `data-default`).
- Data access through **plugins**: Sensors (temps, fan speeds, voltages) and
  Media (playback state).
- Tooling: **WidgetBuilder CLI** (`icuewidget init/validate/package`) plus a
  shipped **AI skill** "to prepare coding agents with specifications and
  conventions."
- Distribution: **Elgato Marketplace** open widget platform launched May
  2026 with iCUE 5.44; creator portal with **free and paid** community
  widgets planned. Elgato's ecosystem director frames it explicitly as the
  Stream Deck platform playbook applied to screens. ([corsair.com press release](https://www.corsair.com/newsroom/press-release/corsair-launches-open-widget-platform-for-the-xeneon-edge-14-5-lcd-touchscreen-on-elgato-marketplace))

### iCUE Nexus (legacy, instructive)

640x48 touch strip; screens hold up to six buttons/widgets (macros, app
launchers, sensor readouts), custom backgrounds (color/image/GIF),
import/export of screen layouts, downloadable game-specific screens, edits
apply live without a save step. Widgets were fixed ~100x48 tiles — reviewers
noted some widgets didn't fit their cells. Quietly discontinued; patch notes
stopped mentioning it. ([techpowerup.com](https://www.techpowerup.com/review/corsair-icue-nexus/4.html), [forum.corsair.com](https://forum.corsair.com/forums/topic/189905-icue-nexus-discontinued/))

## Customization Model

- **Per-device screen config, not per-rig.** Each LCD is configured from its
  device page in iCUE (Screen Setup → presets / Constructor / Hardware
  Screen). There is no notion of one "face theme" spanning multiple screens.
- **Preset-first UX.** Presets up front; Constructor (sensor face builder)
  and widget dashboards are the power path. Constructor is forms-based
  (pick sensors, labels, colors), not freeform drag-drop.
- **Widget dashboards are slot grids.** On the Edge you arrange widgets into
  fixed slot sizes across pages; KitGuru called placement "a little
  restrictive with grid layout system" and noted "no quick way to switch
  between widget and 'normal' display mode." ([kitguru.net](https://www.kitguru.net/peripherals/mat-mynett/corsair-xeneon-edge-review-14-5-touchscreen/))
- **Resolution handling**: 480x480 assets are center/zoom/rotate-fitted at
  import; GIFs above 30fps are rejected or resampled. Widget SDK pushes
  responsive layout across named slot sizes rather than free scaling.
- **Formats**: PNG/BMP/JPG/JPEG/GIF on USB-class screens. The DP-class
  screens accept anything (they're monitors) — iCUE's Image/Video and
  iFrame/Web URL widgets cover video and web content there.
- **Offline behavior**: one hardware face stored on-device; everything
  dynamic dies with iCUE.

## Murals / Scenes — screens vs lighting coordination

- **iCUE Murals** (since iCUE 4.30, Nov 2022): place your RGB devices on a
  2D canvas over an image, video, audio visualization, or live monitor
  mirror; the lighting samples whatever is under each device. Extends to
  Philips Hue, Nanoleaf, and Govee smart lights. ([corsair.com](https://www.corsair.com/us/en/s/icue-murals-lighting), [techpowerup.com](https://www.techpowerup.com/304620/corsair-launches-icue-murals-lighting-a-state-of-the-art-rgb-customization-software))
- **Critical gap: Murals does not drive the LCDs.** No official material or
  review shows pump/Edge screens participating in a Mural. Screen content is
  configured entirely separately per device. Lighting-from-screen-content
  and content-on-screens are two disconnected systems in iCUE. (Observation
  from all sources reviewed 2026-06-09; no counterexample found.)
- The spatial place-devices-over-a-canvas model is the same idea as
  Hypercolor's SpatialEngine — Corsair shipped it for LEDs but never
  promoted screens into that canvas.

## Community Sentiment

Praise:

- The 480x480 IPS panels themselves review well (bright, crisp, "gorgeous
  IPS display" — Windows Central on H150i Elite LCD).
- Giphy integration and the community GIF gallery made personalization easy;
  Corsair runs an official ELITE LCD GIF gallery forum. ([forum.corsair.com](https://forum.corsair.com/gallery/category/4-elite-lcd-gif-gallery/))
- Xeneon Edge landed well ($249.99 felt fair; KitGuru 8.5/10, PCWorld
  positive) — mounting flexibility (desk/tripod/radiator/magnet) is loved.
- The open widget platform announcement (May 2026) was received positively
  across the tech press.

Complaints / limitations users hit:

- **iCUE dependency**: screens fall back to a single static face without
  iCUE running; iCUE itself is heavy and historically buggy (sensor
  disappearance threads across 5.x: [forum.corsair.com 188203](https://forum.corsair.com/forums/topic/188203-missing-sensors-from-icue-since-51195/), [184997](https://forum.corsair.com/forums/topic/184997-all-sensors-missing-after-updating-icue-to-version-52128/)).
- **Sensor lock-in**: Corsair devices expose sensors to only one client, so
  iCUE fights AIDA64/HWiNFO — users must choose iCUE control or third-party
  monitoring, not both. ([forum.corsair.com](https://forum.corsair.com/forums/topic/167308-icue-and-aida64/), [forums.aida64.com](https://forums.aida64.com/topic/7399-aida64-not-detecting-any-sensors-for-corsair-h150i-elite-capellix-icue-controller-core/))
- **Content ceiling on 480x480**: no video, no webcam, no web content, no
  screen mirror; GIF-only animation at ≤30fps; users report config limits
  (e.g. only two GIFs in a config: [forum.corsair.com 184332](https://forum.corsair.com/forums/topic/184332-gif-display-on-elite-lcd-issue/)).
- **Constructor depth**: max three sensors, label/color tweaks only — no
  fonts, gauges, charts, or layout freedom on pump faces.
- Screen bugs: TITAN RX LCD flicker on startup ([forum.corsair.com 194160](https://forum.corsair.com/forums/topic/194160-screen-issue-on-the-icue-link-titan-360-rx-lcd-cpu-cooler/)), screens reverting to defaults (fixed only in 5.46.67, June 2026).
- **Ecosystem lock-in**: PC Gamer's verdict on the TITAN RX LCD — don't buy
  unless you're all-in on iCUE LINK. ([pcgamer.com](https://www.pcgamer.com/hardware/cooling/corsair-icue-link-titan-360-rx-lcd-review/))
- Xeneon Edge: grid-locked widget placement, brightness only adjustable in
  software, fullscreen-app focus issues, no USB passthrough, Nexus owners
  burned by quiet abandonment.
- **No Linux support at all** for any screen content.

## Third-Party Tools People Use Instead

- **OpenLinkHub** ([github.com/jurkovic-nikola/OpenLinkHub](https://github.com/jurkovic-nikola/OpenLinkHub)) — open-source
  Linux web-dashboard for iCUE LINK hubs and Corsair AIOs; 100+ devices;
  LCD modes (CPU/GPU/liquid temp, pump speed, combined metrics) plus custom
  images/animations stored in `database/lcd/images/`. The de facto Linux
  answer; the closest existing competitor-adjacent project to Hypercolor's
  faces on Corsair hardware. ([gamingonlinux.com](https://www.gamingonlinux.com/2025/07/openlinkhub-is-an-open-source-interface-to-manage-icue-link-hub-and-various-corsair-devices-on-linux/))
- **liquidctl** — Elite LCD support requested ([issue #408](https://github.com/liquidctl/liquidctl/issues/408)) but LCDs
  largely unsupported; pump/fan control only.
- **FanControl.CorsairLink** — Windows fan control without iCUE; no LCD.
- **SignalRGB** — users request Corsair LCD support; on the roadmap, not
  shipped as of March 2025. ([forum.signalrgb.com](https://forum.signalrgb.com/t/corsair-h150i-lcd-custom-screen-support/4130))
- **Community widget libraries** (post-SDK): [SilverFuel/xeneon-widgets](https://github.com/SilverFuel/xeneon-widgets)
  (dashboard, clock, sysmon, weather, network, media, calendar),
  [efebiskin/xeneon-widgets](https://github.com/efebiskin/xeneon-widgets) (Pomodoro, Hacker News, crypto, Spotify),
  [imnotStealthy/icue-edge-widgets](https://github.com/imnotStealthy/icue-edge-widgets) (ISS tracker, GitHub repo monitor,
  habit rings — with `pump_lcd` manifests targeting watercooling screens).
- On DP-class screens (Edge, 5" modules), people simply bypass iCUE with
  **AIDA64 SensorPanel / Rainmeter / any window**, since it's a real monitor.

## What's Worth Stealing

- **HTML widgets as the face substrate.** Corsair validated Hypercolor's
  exact bet: web tech on an embedded Chromium is the right authoring model
  for screen faces. Their `manifest.json` + device-target +
  responsive-slot-sizes scheme (`pump_lcd` / `dashboard_lcd` /
  `keyboard_lcd`) is a clean pattern for one face shipping across round
  480x480, portrait 720x1280, and wide strips.
- **CLI scaffolding + AI skill for face authors.** `icuewidget
  init/validate/package` plus a shipped agent skill is exactly the
  hyperskills-flavored DX we'd want for face development.
- **Property declaration for user-tweakable controls** (`x-icue-property`
  meta tags with type/label/default) — maps directly onto Hypercolor's
  live-controls system; faces should expose controls the same way effects do.
- **Hardware-screen fallback**: flash one face to device memory so the
  screen isn't dead when the daemon is down. Users notice and value this.
- **Giphy/community galleries**: low-friction content acquisition mattered
  more to user joy than deep editors. A face gallery with one-click install
  is high leverage.
- **Sensor faces as the killer app**: coolant/CPU/GPU temp readouts are the
  #1 real use. Ship gorgeous sensor faces first, memes second.
- **Constructor's layering model** (background media + sensor overlay with
  styled labels) is the right minimal editor; ours can go further (fonts,
  gauges, more than 3 sensors) and immediately out-feature it.
- **Marketplace ambition**: Corsair/Elgato are explicitly running the Stream
  Deck platform play on screens. An open, non-paywalled face ecosystem is a
  differentiator Hypercolor gets for free by being open source.

## What to Avoid

- **Two disconnected worlds.** iCUE's biggest structural flaw: Murals
  (spatial lighting) and screen content never meet. Hypercolor's faces
  should be first-class citizens of the same canvas/scene system as LED
  zones — a face can be a spatially-sampled surface or display scene-aware
  content. Nobody does this today.
- **Software-dead screens.** Don't let a face exist only while a config UI
  runs; the daemon-owned render loop already avoids iCUE's worst failure
  mode, keep it that way.
- **Sensor monopolies.** iCUE's exclusive sensor locks force users to choose
  between tools. Expose sensors openly; never fight HWiNFO-equivalents.
- **Grid-locked layout with no escape hatch.** Slot grids are fine as a
  default, but reviewers chafed immediately; offer freeform positioning.
- **Format gatekeeping on capable hardware.** GIF-only at 30fps on a 600-nit
  IPS panel reads as artificial; if the panel and link can take a video
  stream, feed it one (Hypercolor's Servo/canvas pipeline already renders
  arbitrary content — don't nerf the output stage).
- **Quiet product abandonment** (Nexus): if a face/device class is
  deprecated, say so; the community remembers.
- **Windows-only assumptions** in the face SDK — Corsair's entire screen
  stack ignores Linux, which is precisely Hypercolor's opening.

## Source Index (accessed 2026-06-09)

- https://www.corsair.com/us/en/p/cpu-coolers/cw-9060075-ww/icue-h150i-elite-lcd-xt-display-liquid-cpu-cooler
- https://help.corsair.com/hc/en-us/articles/4412131516813-ELITE-LCD-How-to-set-up-your-ELITE-LCD-in-iCUE-4-or-newer
- https://help.corsair.com/hc/en-us/articles/4420187201293-ELITE-LCD-What-to-do-if-your-ELITE-LCD-Display-only-shows-the-hardware-screen
- https://www.corsair.com/uk/en/explorer/diy-builder/cpu-coolers/create-a-custom-lcd-screen-for-your-aio-in-icue/
- https://www.corsair.com/us/en/explorer/diy-builder/cpu-coolers/corsair-icue-link-lcd-aio-coolers-and-icue-link-lcd-upgrade-kit-everything-you-need-to-know/
- https://www.corsair.com/us/en/explorer/diy-builder/cpu-coolers/corsair-icue-link-5-inch-lcd-screen-module-everything-you-need-to-know/
- https://www.corsair.com/us/en/explorer/diy-builder/cpu-coolers/icue-link-titan-ii-ultra-360-lx-lcd-aio-everything-you-need-to-know/
- https://rkiologist.com/2026/06/04/corsair-shows-off-the-icue-link-titan-ii-ultra-360-lx-lcd-at-computex-2026/
- https://www.corsair.com/newsroom/press-release/corsair-announces-exciting-new-hardware-for-expanded-comfort-cooling-and-customization-at-computex-2026
- https://www.corsair.com/us/en/explorer/diy-builder/accessories/xeneon-edge-widgets-explained/
- https://www.corsair.com/us/en/explorer/diy-builder/accessories/how-to-create-a-custom-widget-for-the-xeneon-edge/
- https://docs.elgato.com/icue/widgets/
- https://www.corsair.com/newsroom/press-release/corsair-launches-open-widget-platform-for-the-xeneon-edge-14-5-lcd-touchscreen-on-elgato-marketplace
- https://www.kitguru.net/peripherals/mat-mynett/corsair-xeneon-edge-review-14-5-touchscreen/
- https://www.pcworld.com/article/2888302/corsair-xeneon-edge-14-5-review.html
- https://www.tomshardware.com/tech-industry/corsair-builds-multi-function-touchscreen-lcd-into-a-usd400-case-frame-4000d-enclosure-gets-a-modular-xeneon-edge-upgrade
- https://www.corsair.com/us/en/explorer/gamer/keyboards/vanguard-pro-96/
- https://www.techpowerup.com/review/corsair-icue-nexus/4.html
- https://forum.corsair.com/forums/topic/189905-icue-nexus-discontinued/
- https://www.corsair.com/us/en/s/icue-murals-lighting
- https://www.techpowerup.com/304620/corsair-launches-icue-murals-lighting-a-state-of-the-art-rgb-customization-software
- https://www.corsair.com/us/en/explorer/release-notes/icue/icue-54667/
- https://www.vortez.net/news_story/corsair_rolls_out_icue_5_36_update_with_new_widgets_and_major_stability_fixes.html
- https://x.com/CORSAIR/status/2000736212189954456
- https://www.pcgamer.com/hardware/cooling/corsair-icue-link-titan-360-rx-lcd-review/
- https://forum.corsair.com/forums/topic/167308-icue-and-aida64/
- https://forum.signalrgb.com/t/corsair-h150i-lcd-custom-screen-support/4130
- https://github.com/jurkovic-nikola/OpenLinkHub
- https://www.gamingonlinux.com/2025/07/openlinkhub-is-an-open-source-interface-to-manage-icue-link-hub-and-various-corsair-devices-on-linux/
- https://github.com/liquidctl/liquidctl/issues/408
- https://github.com/SilverFuel/xeneon-widgets
- https://github.com/efebiskin/xeneon-widgets
- https://github.com/imnotStealthy/icue-edge-widgets
- https://forum.corsair.com/forums/topic/174656-videos-on-lcd-cooler/
- https://forum.corsair.com/forums/topic/184332-gif-display-on-elite-lcd-issue/
- https://forum.corsair.com/forums/topic/194160-screen-issue-on-the-icue-link-titan-360-rx-lcd-cpu-cooler/
