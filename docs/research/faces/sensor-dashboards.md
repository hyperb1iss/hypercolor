# Sensor Dashboards & Secondary Displays: Competitive Landscape (June 2026)

Research for Hypercolor "display faces" — composable dashboards rendered to small
hardware LCDs (AIO pump caps, fan hubs, USB smart screens, keyboard OLEDs).
All facts dated as of June 2026 unless noted. Sources inline.

## TL;DR

- **The format that wins is declarative data-binding**: every successful ecosystem
  separates *data sources* (sensors) from *visual elements* (widgets) and binds them
  by name in a declarative file — Rainmeter's measures/meters INI (alive since 2001,
  v4.5.26 shipped May 20, 2026), Turing's `theme.yaml`, Stream Deck's layout JSON,
  GameSense's handler JSON. Hypercolor faces should be a declarative document with
  named sensor bindings, not code.
- **The widget taxonomy is tiny and universal**: text, bar, radial/gauge, line graph
  (history), image/pixmap, shape. Every ecosystem converges on these six. AIDA64 adds
  multi-state custom gauge images; Stream Deck adds z-ordered compositing primitives.
- **Community sharing is the moat**: AIDA64's forum threads, Turing's GitHub
  Discussions, Rainmeter's DeviantArt/Reddit, Wallpaper Engine's 1M+ workshop items.
  A single shareable file (`.SENSORPANEL`, `theme.yaml`, `.rmskin`) with embedded
  assets is the table stakes; a browsable gallery is the growth engine.
- **Nobody owns Linux**. AIDA64, InfoPanel, Wallpaper Engine, SignalRGB, Armoury
  Crate, L-Connect are Windows-only or Windows-first. turing-smart-screen-python and
  liquidctl/coolercontrol are the only meaningful Linux players, and neither has a
  real visual editor. This is Hypercolor's opening.
- **Vendor software is the cautionary tale**: ASUS and Lian Li ship fixed-template
  monitor themes with image/GIF upload and almost no layout freedom. Tom's Hardware
  on Lian Li TL LCD: "not as configurable as I'd like."

---

## 1. Turing Smart Screen + turing-smart-screen-python

**Hardware** (per [project README](https://github.com/mathoudebine/turing-smart-screen-python)):
cheap USB-C IPS serial displays sold as Turing/TURZX, XuanFang, UsbPCMonitor, Kipye
Qiye, WeAct Studio. Sizes 0.96" to 12.3"; the iconic SKUs are the 3.5" (480x320),
5" (800x480), and 8.8" ultrawide (1920x480). Not real monitors — framebuffers pushed
over USB serial, no GPU output needed. Some have backplate RGB LEDs.

**Project status**: 2.1k stars, 365 forks, 44 releases; v3.10.0 released April 2026
with expanded device support. Multi-OS: Windows, Linux, macOS, Raspberry Pi. Works as
both a standalone system monitor and an abstraction library.
([releases](https://github.com/mathoudebine/turing-smart-screen-python/releases))

**Theme format** ([wiki: System monitor themes](https://github.com/mathoudebine/turing-smart-screen-python/wiki/System-monitor-:-themes)):
a single `theme.yaml` per theme with sections:

- `display`: target size, orientation, backplate LED RGB.
- `static_images` / `static_text`: background art and fixed labels, painter's-order,
  positioned by X/Y/WIDTH/HEIGHT, font file + size + color, ALIGN, and
  BACKGROUND_IMAGE crop-sampling to fake transparency.
- `STATS`: the data-binding tree, organized by sensor domain (CPU, GPU, MEMORY,
  DISK, NETWORK, DATE, UPTIME, CUSTOM), each with a per-sensor refresh `INTERVAL`.

Binding example — sensor path implies the source, child keys are widget instances:

```yaml
CPU:
  PERCENTAGE:
    TEXT:
      SHOW: True
      X: 100
      Y: 20
      FONT: roboto-mono/RobotoMono-Bold.ttf
      FONT_SIZE: 20
      FONT_COLOR: 255, 255, 255
```

**Widgets**: `TEXT`, `GRAPH` (horizontal bar with MIN/MAX, BAR_COLOR, outline),
`RADIAL` (circular progress: RADIUS, ANGLE_START/END/STEPS, CLOCKWISE), `LINE_GRAPH`
(history chart: HISTORY_SIZE, AUTOSCALE, LINE_COLOR, AXIS). Custom data sources are
Python classes (`CustomDataSource` in `sensors_custom.py`) referenced from a
`CUSTOM:` section — extensibility without touching the renderer.

**Community**: themes shared via [GitHub Discussions "Themes" category](https://github.com/mathoudebine/turing-smart-screen-python/discussions/categories/themes)
— 5+ pages of community themes per display size. Hits: CyberArasaka (34 comments),
Cyberdeck (31), Fallout Pip-Boy (16 comments / 12 votes), Spotify player themes,
Proxmox server panels. A bundled theme editor lowers the creation bar.

**Why it's popular**: dirt-cheap hardware + free open software + a theme format
simple enough to hand-edit but visual enough to screenshot-flex. The README's theme
gallery (`res/themes/themes.md`) doubles as marketing.

## 2. AIDA64 SensorPanel — the gold standard

**Editor** ([AIDA64 SensorPanel](https://www.aida64.com/aida64-sensorpanel),
[creation guide](https://aida64.co.uk/knowledgebase/how-to-create-a-fully-customized-sensorpanel-in-aida64)):
the SensorPanel Manager is a drag-and-drop layout editor. Item types: images,
text/labels, sensor value items, **gauges**, **graphs** (time series with min/max
thresholds), **bars** (threshold color shifts green→yellow→red). Exact X:Y
positioning, arrow nudging, multi-select "Modify" for bulk edits, graph overlaying
(e.g. all CPU cores on one grid). Panel size is freeform (e.g. 500x145 or a full
800x480 LCD). No transparent backgrounds — users fake it with desktop screenshots.

**Custom gauges**: users can draw and upload up to **15 indicator states** as
individual images, then select "Custom" as gauge type. This one feature powers most
of the community's visual diversity — analog needles, fantasy dials, themed arcs.

**Sharing**: Export writes a single `.SENSORPANEL` file containing *all graphics and
settings*; Import restores it wholesale
([SensorPanel Manager manual](https://www.aida64.com/user-manual/sensorpanel/sensorpanel-manager)).
Community lives in the [official forum "Share your Sensorpanels" thread](https://forums.aida64.com/topic/13296-share-your-sensorpanels/)
(years of activity) plus a curated [gallery on aida64.com](https://www.aida64.com/sensorpanels)
and an Overclock.net megathread.

**LCD reach**: beyond the on-desktop panel, the AIDA64 LCD module drives **50+
external display families** — Turing (2.1"/3.5"/5"/7"/8.8" up to 1920x480),
BeadaPanel, Matrix Orbital (LK/GLK/EVE2/GTT), Logitech, Razer, etc. The
[AIDA64 LCD Guide PDF](https://download.aida64.com/resources/lcd/aida64_lcd_guide.pdf)
was last updated **January 16, 2026** — actively maintained.
**RemoteSensor LCD** embeds a web server so any phone/tablet browser becomes a panel
([forum thread](https://forums.aida64.com/topic/2636-remotesensor-lcd-for-smartphones-and-tablets/));
layouts for it are importable files too.

**Why it's the standard**: deepest sensor coverage in the industry, a 15-year-old
editor that never broke users' panels, and a single-file share format. Weaknesses:
Windows-only, paid license, dated editor UX, raster-only widgets.

## 3. Rainmeter — declarative skins, 25 years old

**Status**: launched 2001, still shipping — v4.5.26 released **May 20, 2026**
([releases](https://github.com/rainmeter/rainmeter/releases),
[rainmeter.net](https://www.rainmeter.net/)).

**Architecture** ([manual](https://docs.rainmeter.net/manual/),
[skin anatomy](https://docs.rainmeter.net/manual/getting-started/skin-anatomy/)):
a skin is a plain INI file. The hard separation that defines the system:

- **Measures** gather data (CPU, Memory, NetIn/Out, FreeDiskSpace, Time, Calc
  formulas, WebParser for HTTP+regex scraping, Plugin for native extensions).
  Each has a string value and a number value, MinValue/MaxValue for percentage
  scaling, `Substitute` regex post-processing, and `IfCondition`/`IfAction`
  threshold triggers ([measures](https://docs.rainmeter.net/manual/measures/)).
- **Meters** display data, bound via `MeasureName=`. Ten types: `Bar`, `Bitmap`
  (sprite-sheet frames), `Button`, `Histogram`, `Image`, `Line`, `Rotator` (rotate
  an image by value — analog needles), `Roundline` (radial arc fill), `Shape`
  (full vector graphics: paths, fills, strokes), `String`
  ([meters](https://docs.rainmeter.net/manual/meters/)).
- **Update loop**: `[Rainmeter]` section sets an Update rate in ms; measures evaluate
  sequentially each tick, then meters redraw. Per-measure/meter divisors tune
  cadence.
- **MeterStyle** sections give CSS-like style inheritance; `[Variables]` +
  `#Var#` substitution and dynamic variables provide templating; **bangs**
  (`!SetOption`, `!ShowMeter`) are the imperative escape hatch, and **Lua script
  measures** ([docs](https://docs.rainmeter.net/manual/lua-scripting/)) handle logic
  too awkward for INI, including inline Lua expressions.

**Why it survived 15+ years**: the INI is hand-editable with instant refresh (edit →
right-click refresh → see it); the measure/meter split means data and presentation
evolve independently; `.rmskin` packaging made sharing one-click; and the community
(forums, [r/Rainmeter](https://www.reddit.com/r/Rainmeter/), DeviantArt, Discord)
treats skins as an art form. Massive suites (Mond, JaxCore) are still actively
ranked in 2026 roundups
([example](https://rainmeterpro.in/blog/top-20-best-rainmeter-skins-suites-for-the-ultimate-desktop-setup-2026)).
Weaknesses: Windows-only, single-threaded update model, INI verbosity at scale, no
official marketplace (discovery is scattered).

## 4. Wallpaper Engine — accessible authoring at scale

**Scale**: "over a million wallpapers" on Workshop per the
[Steam store page](https://store.steampowered.com/app/431960/Wallpaper_Engine/);
20–50M owners ([SteamSpy](https://steamspy.com/app/431960)); 98% positive of ~227k
reviews. Version 2.7 (Sept 4, 2025) shipped particle editor upgrades, user
shortcuts, and image filters
([patch notes](https://steamdb.info/patchnotes/19812784/)).

**Editor architecture** ([designer docs](https://docs.wallpaperengine.io/en/scene/overview.html)):
three content types — **scene** (the real editor: layered 2D/3D compositions),
**video**, **web** (HTML/JS). Scene wallpapers compose image/text/particle/sound
layers with an effects stack per layer, puppet-warp skeletal animation, timeline
animations, lighting, and fully custom HLSL-style shaders
([shader docs](https://docs.wallpaperengine.io/en/scene/shader/overview.html)).
**SceneScript** (JavaScript-like) scripts properties and events. Audio reactivity is
first-class: shaders receive `g_AudioSpectrum16Left/Right` (also 32- and 64-band)
FFT arrays ([shader variables](https://docs.wallpaperengine.io/en/scene/shader/variables.html)).

**User properties — the key innovation**
([docs](https://docs.wallpaperengine.io/en/scene/userproperties/overview.html)):
creators expose a typed settings schema to end users: `color`, `slider`, `bool`,
`combo`, `textinput`, `texture` (user swaps an image/video), `usershortcut`, and
collapsible `group`s, with declarative visibility conditions
(`propertykey.value == "x"`). Properties bind to layer/effect/shader uniforms.
This lets one workshop item serve thousands of personalized variants without forking.

**Why content creation is accessible**: drag-and-drop import of any image/video,
progressive depth (start with a JPEG + one effect, end with custom shaders), and
Workshop sharing that auto-uploads dependencies (textures, shaders, child particles)
([asset sharing docs](https://docs.wallpaperengine.io/en/scene/assets/sharing.html)).

## 5. Small-display SDKs: Stream Deck & GameSense

### Elgato Stream Deck

The most polished SDK model for tiny dynamic surfaces
([SDK docs](https://docs.elgato.com/streamdeck/sdk/guides/keys/)):

- **Keys**: plugin actions own key faces; images are SVG (recommended), PNG, or GIF;
  states declared in a manifest; runtime updates via `setImage`/`setTitle`.
- **Touchscreen layouts** ([touch strip layout reference](https://docs.elgato.com/streamdeck/sdk/references/touch-strip-layout/)):
  Stream Deck+ gives each action a 200x100 px strip slot rendered from a **layout
  JSON** (`$schema: https://schemas.elgato.com/streamdeck/plugins/layout.json`).
  Four item types — `text`, `pixmap` (file/base64/SVG), `bar`, `gbar` (bar with
  indicator triangle) — each with `key` (update handle), `rect [x,y,w,h]`, `zOrder`
  (0–700), `opacity`, `enabled`, gradients, bar `subtype` (Rectangle,
  DoubleRectangle, Trapezoid, DoubleTrapezoid, Groove), text overflow modes.
  Plugins push partial updates with `setFeedback({key: value})`; structure
  (`rect`/`type`/`key`) is immutable at runtime — only values/colors change.
  Predefined layouts (`$B1`, `$B2`...) cover the common icon+value+bar pattern.
- **Distribution**: the [Elgato Marketplace](https://marketplace.elgato.com/stream-deck)
  hosts plugins and icon packs (mostly free, some paid).

The layout-JSON + keyed-feedback split is the cleanest "declarative face, streaming
data" contract in this whole survey.

### SteelSeries GameSense

OLED/LCD screen handlers for keyboards/mice/Arctis
([screen handler docs](https://github.com/SteelSeries/gamesense-sdk/blob/master/doc/api/json-handlers-screen.md)):
apps POST JSON events to a local Engine REST endpoint; handlers declare
`device-type: screened` (or `screened-128x36` etc.), a zone, and frame definitions —
text lines with `prefix`/`suffix`/`bold`/`wrap`, `has-progress-bar` (0–100), or raw
1-bit `image-data` byte arrays; animations via `length-millis` + `repeats`. Since
Engine 3.17.9, events can carry dynamic `image-data-WIDTHxHEIGHT` payloads.
Resolution-keyed assets (128x36/40/48/52) show how to target heterogeneous tiny
screens from one handler. Largely in maintenance mode but a clean minimal model.

## 6. Vendor software: ASUS & Lian Li

### ASUS (Armoury Crate)

- **AniMe Matrix** (mini-LED dot matrices on Zephyrus laptops, motherboards, Delta S
  Animate headset, Strix Flare Animate keyboard;
  [customization guide](https://rog.asus.com/articles/guides/how-to-customize-the-anime-matrix-on-your-rog-laptop-motherboard-keyboard-or-headset/)):
  three modes — Animation (premade + custom loops with per-segment speed), System
  (notifications/system stats), Audio (visualizer + track info). A browser-based
  **Pixel Editor** does frame-by-frame drawing and image import; scrolling text
  overlays; custom boot/shutdown animations. **AniMe Matrix Sync** coordinates
  effects across all matrix-equipped gear
  ([announcement](https://rog.asus.com/us/articles/product-news/anime-matrix-sync-brings-dazzling-coordinated-visual-effects-to-your-gaming-pc/)).
- **Ryujin II/III AIO LCD** ([setup FAQ](https://www.asus.com/support/faq/1048625/),
  [forum](https://rog-forum.asus.com/t5/armoury-crate/ryujin-iii-aio-lcd-hardware-monitor-theme/td-p/993310)):
  upload images/GIFs, or pick a **fixed hardware-monitor theme** (Galactic,
  Cyberpunk, "custom" = recolor). Users cannot compose their own sensor layouts —
  forum threads are full of requests for more monitor themes.

### Lian Li (L-Connect 3)

[UNI FAN TL LCD](https://lian-li.com/product/uni-fan-tl-lcd/): 1.6" 400x400 IPS
screens in each fan hub. Users upload MP4/GIF/JPG/PNG or pick built-in hardware
monitor dashboards (CPU/GPU temp & load, fan speed). Lian Li explicitly recommends
**max 3 sensor displays** because "the more sensors used, the higher the CPU load"
— the software renders per-fan streams on the host. L-Connect 3 changelogs
([changelog](https://lian-li.com/l-connect3/l3-changelog/)) added 3-group/4-group
sensor modes through 2025. Tom's Hardware verdict: heads-turning but
["not as configurable as I'd like"](https://www.tomshardware.com/pc-components/cooling/hands-on-lian-lis-lcd-screen-fans-turn-heads-and-are-surprisingly-affordable-but-not-as-configurable-as-id-like)
— fixed templates, no community format, no layout editor.

Both vendors prove demand for monitor faces on tiny LCDs, and both leave layout
freedom and community sharing completely unserved.

## 7. Open projects driving cheap LCDs

- **InfoPanel** ([github.com/habibrehmansg/infopanel](https://github.com/habibrehmansg/infopanel),
  GPL-3.0, C#/WPF, ~219 stars, active through 2026; [infopanel.net](https://infopanel.net/)):
  the strongest open Windows player. Sensors from **HWiNFO shared memory** or
  LibreHardwareMonitor; drag-and-drop profile designer; widgets: text, gauges,
  graphs, bars, **donuts**, images/GIFs; multiple profiles per display; renders to
  desktop overlays and USB panels — **BeadaPanel**, Turing/TURZX (models A/C/E),
  Thermalright. Plugin API with community plugins (Spotify, FPS counters, weather).
  Community on Reddit + Discord. An unofficial Linux port
  ([InfoPanel-linux](https://github.com/emaspa/InfoPanel-linux), Avalonia) exists.
  Note: the prompt's "beadi" almost certainly meant **BeadaPanel** (NXELEC's USB
  media-link panels, 7" kits); the only GitHub "Beadi" is an unrelated node editor
  for buttplug.io devices, and no "ZZZ" deck-display project surfaced in searches.
- **turing-smart-screen-python** — covered in §1; the de facto Linux answer.
- **liquidctl / coolercontrol** ([liquidctl](https://github.com/liquidctl/liquidctl),
  [Kraken LCD PR #479](https://github.com/liquidctl/liquidctl/pull/479)): Linux
  control of NZXT Kraken Z LCDs (image/GIF upload). Community glue scripts like
  [NZXT-Kraken-Linux-Infographic](https://github.com/aminedeesucre/NZXT-Kraken-Linux-Infographic)
  render a 320x320 PNG with Pillow every few seconds and push it via liquidctl — a
  systemd-timer hack standing in for the product Hypercolor could be.
- **Seeed usbdisp** ([seeed-linux-usbdisp](https://github.com/Seeed-Studio/seeed-linux-usbdisp)):
  Linux kernel driver turning USB displays into real framebuffers.
- **SignalRGB** (closest commercial RGB-engine analog, v2.5.28 Jan 2026, 3,500+
  devices): treats TURZX LCDs as RGB canvas zones; users are
  [requesting real monitor dashboards](https://forum.signalrgb.com/t/system-monitoring-monitor-dashboards/3334)
  — unshipped as of June 2026. Windows-only.

---

## Theming & Format Architectures (cross-cutting)

| Ecosystem | Format | Data binding model | Logic escape hatch |
|---|---|---|---|
| Rainmeter | INI sections | meter `MeasureName=` → measure | Lua scripts, bangs |
| Turing python | `theme.yaml` | sensor path → widget subtree | custom Python sensor classes |
| AIDA64 | binary `.SENSORPANEL` | editor-managed item↔sensor refs | none (closed) |
| Wallpaper Engine | scene JSON + user-properties schema | properties → layer/shader uniforms | SceneScript (JS-like), HLSL |
| Stream Deck | layout JSON (`$schema`-validated) | `setFeedback` by item `key` | full plugin runtime (TS/any) |
| GameSense | handler JSON | event value → frame templates | GoLisp handlers |
| InfoPanel | app profiles | editor-managed | C# plugin API |

Convergent lessons:

1. **Named-element + partial-update is the right wire contract.** Stream Deck's
   immutable structure / mutable values split maps perfectly onto "face definition
   document + streaming sensor channel."
2. **Sensor identity must be path-like and stable** (`CPU.PERCENTAGE`,
   `GPU[0].TEMP`) so themes survive hardware changes. Rainmeter's measure names and
   Turing's STATS tree both do this; AIDA64's editor hides it but the same model is
   underneath.
3. **Per-binding refresh intervals matter on small displays** — Turing's `INTERVAL`
   per sensor and Rainmeter's UpdateDivider both exist because uniform refresh
   wastes cycles (cf. Lian Li's "max 3 sensors" nerf, which is what happens when
   you don't architect for this).
4. **Typed user-property schemas** (Wallpaper Engine) are how one theme serves many
   users — expose palette, units (°C/°F), visible panels as declared properties
   instead of forks. Hypercolor's effect controls system is already this.
5. **Single-file portability with embedded assets** (`.SENSORPANEL`, `.rmskin`,
   workshop dependency bundling) is non-negotiable for community sharing.

## Widget Taxonomy (union across ecosystems)

- **Text/value label** — font, size, color, align, prefix/suffix unit formatting
  (GameSense's `prefix`/`suffix` is a nice touch), overflow handling
  (clip/ellipsis/fade from Stream Deck).
- **Bar** — horizontal/vertical fill, min/max range, threshold color zones
  (AIDA64's green→yellow→red), border/outline, subtype shapes (Stream Deck's five).
- **Radial/gauge** — arc fill (Turing RADIAL: start/end angle, steps, direction;
  Rainmeter Roundline), needle rotation over face image (Rainmeter Rotator), and
  **multi-state image gauges** (AIDA64's 15-frame custom gauges; Rainmeter Bitmap).
  Donut variant (InfoPanel).
- **Line graph / histogram** — value history ring buffer (Turing HISTORY_SIZE,
  AUTOSCALE; Rainmeter Histogram with two-measure overlay; AIDA64 multi-series
  core graphs).
- **Image/pixmap** — static art, GIF/video loops (Lian Li MP4, Ryujin GIF), SVG
  (Stream Deck), sprite sheets (Rainmeter Bitmap).
- **Shape/vector** — paths, rounded rects, strokes (Rainmeter Shape meter is the
  benchmark; everything else is raster).
- **Specials**: audio visualizer (AniMe Matrix audio mode, WE spectrum arrays),
  scrolling ticker text (AniMe Matrix), progress-over-icon composites (Stream Deck
  `$B1`), clock/date/uptime formatters (universal).

## Community & Marketplace Dynamics

- **AIDA64**: forum-thread sharing since ~2014, official curated gallery; longevity
  driven by format stability — decade-old panels still import.
- **Turing python**: GitHub Discussions as marketplace; gallery markdown in-repo;
  pop-culture themes (Pip-Boy, cyberpunk) dominate — fandom skins are the demand
  driver, not utilitarian dashboards.
- **Rainmeter**: DeviantArt + r/Rainmeter + forums; mega-suites with their own
  configurators; surviving on hand-editability and a packaging format. No central
  store after 25 years — discovery pain is its biggest community complaint.
- **Wallpaper Engine**: Steam Workshop's one-click subscribe + automatic dependency
  bundling + typed user properties = 1M+ items and 20–50M owners. The lesson:
  **lower the floor (drag-drop media), raise the ceiling (shaders/scripts), and let
  consumers customize without forking.**
- **Stream Deck**: first-party Marketplace with paid content — proof small-display
  content can be a commercial ecosystem.

## What's Worth Stealing for Hypercolor Faces

1. **Declarative face document = Turing's theme.yaml structure + Stream Deck's
   layout JSON rigor.** Named widgets with `rect`/z-order/typed props, sensor
   bindings by stable path, JSON-schema validated. Hypercolor already has the
   render loop, compositor, and WebSocket state — faces are "scenes for pixels
   that mean something."
2. **Measures/meters separation (Rainmeter).** Model sensor sources as first-class
   named entities with min/max normalization, per-source refresh interval, and
   string+number duality; widgets bind by name. This also makes faces testable
   with fake sensor feeds.
3. **Threshold semantics in the binding, not the widget** — AIDA64's color zones
   and Rainmeter's IfConditions: `warn_at`/`crit_at` on a binding should restyle
   any widget type consistently.
4. **Multi-state image gauges (AIDA64's 15-state custom gauges).** Cheap to
   implement, unlocks enormous community art energy — needle dials, animated
   mascots, fandom faces.
5. **User-property schemas (Wallpaper Engine).** Faces declare exposed controls
   (palette, units, layout toggles) that surface in the Hypercolor UI exactly like
   effect controls do today. One face, infinite personalizations.
6. **Partial-update protocol (Stream Deck setFeedback).** Face structure is static;
   the daemon streams keyed value updates. Maps directly onto the existing watch-
   channel architecture.
7. **Resolution classes, not fixed sizes (GameSense's `screened-WxH`, Turing's
   per-size theme folders).** Faces should declare aspect/size classes (square 400x400
   AIO, ultrawide 1920x480, tiny OLED 128x40) with normalized coordinates —
   Hypercolor's spatial sampler already thinks in `[0,1]`.
8. **Single-file share format with embedded assets** + a gallery from day one
   (GitHub Discussions is a fine v1, per Turing).
9. **The Linux vacuum is the wedge.** AIDA64/InfoPanel/SignalRGB are Windows-only;
   Linux users are hand-rolling Pillow+liquidctl systemd scripts. A daemon-rendered
   face on a Kraken LCD or Turing panel with a real editor has zero competition.
10. **Audio + ambient reactivity as a differentiator** — WE's spectrum uniforms and
    AniMe Matrix's audio mode show demand; Hypercolor's input pipeline already has
    audio/screen data flowing. No sensor-dashboard product fuses RGB scene state
    with the LCD face. That fusion (face matches the active scene's palette) is
    uniquely ours.

## What to Avoid

- **Fixed-template monitor themes** (ASUS Ryujin's three themes, Lian Li's preset
  dashboards). Templates without composability generate forum begging, not
  community content.
- **Host-side per-device rendering that scales linearly with screens** — Lian Li's
  "keep it to 3 sensors" warning is a self-inflicted nerf. Render faces in the
  existing compositor at producer cadence; one canvas, many samplers.
- **Imperative-only theming.** GameSense GoLisp and pure-code faces gate creation to
  programmers; the winning floor is declarative-with-escape-hatch.
- **Raster-only widget primitives** (AIDA64). Tiny screens vary wildly in DPI and
  aspect; vector-first (like Rainmeter Shape / Stream Deck SVG) keeps faces crisp
  from 128x40 OLED to 1920x480 ultrawide.
- **No central discovery** (Rainmeter's 25-year scattering across DeviantArt/forums).
  Ship the gallery with the feature.
- **Windows-style sensor coupling.** InfoPanel is chained to HWiNFO's shared memory;
  AIDA64 to its own engine. Hypercolor should define its own sensor-path vocabulary
  fed by hwmon/NVML/etc., so faces aren't coupled to any one backend.
- **Breaking theme compat.** AIDA64's decade of importable panels is why its
  community compounds; version the face schema with `#[serde(default)]`-style
  leniency from v1.

## Open Questions

- Whether to support Wallpaper Engine-style shader/script layers inside faces v1,
  or defer to the existing HTML-effect path (Servo) as the escape hatch.
- BeadaPanel media-link protocol support in hypercolor-hal (USB bulk MJPEG/H.264
  streams) — InfoPanel and AIDA64 both support it; worth a protocol-research pass.
- "ZZZ deck displays" from the original brief could not be identified (June 2026
  searches); revisit if a concrete link surfaces.
