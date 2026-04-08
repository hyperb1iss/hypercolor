# Hypercolor: User Personas & Workflow Maps

> Who uses RGB lighting orchestration on Linux, what do they actually do with it, and how do we make them never look back?

---

## Table of Contents

1. [Persona 1: Bliss — The Architect](#persona-1-bliss--the-architect)
2. [Persona 2: Jake — The Plug-and-Play Gamer](#persona-2-jake--the-plug-and-play-gamer)
3. [Persona 3: Luna — The Content Creator](#persona-3-luna--the-content-creator)
4. [Persona 4: Marcus — The Smart Home Orchestrator](#persona-4-marcus--the-smart-home-orchestrator)
5. [Persona 5: Yuki — The Aesthetic Alchemist](#persona-5-yuki--the-aesthetic-alchemist)
6. [Persona 6: Kai — The Plugin Developer](#persona-6-kai--the-plugin-developer)
7. [Persona 7: Sam — The Audio-Reactive Producer](#persona-7-sam--the-audio-reactive-producer)
8. [Persona 8: Robin — The Windows Refugee](#persona-8-robin--the-windows-refugee)
9. [Day-in-the-Life Scenarios](#day-in-the-life-scenarios)
10. [Frustration Map](#frustration-map)
11. [Delight Moments](#delight-moments)
12. [Accessibility Personas](#accessibility-personas)
13. [Persona Priority Matrix](#persona-priority-matrix)

---

## Persona 1: Bliss — The Architect

### Profile

| | |
|---|---|
| **Name** | Bliss |
| **Age** | 28 |
| **Occupation** | Principal Software Engineer |
| **Technical Skill** | Expert — writes kernel modules for fun |
| **OS** | Arch Linux / CachyOS, tiling WM (Hyprland) or GNOME 47 |
| **Editor** | Neovim with SilkCircuit theme |
| **Terminal** | Ghostty |

### Hardware Setup

**PC (custom white build in Lian Li O11 Dynamic EVO):**
- ASUS ROG STRIX Z790-A GAMING WIFI II (AURA RGB)
- ASUS Dual RTX 4070 SUPER White OC (GPU RGB via SMBus)
- G.Skill Trident Z5 Neo RGB DDR5-6000 64GB (2x32GB)
- Corsair iCUE LINK System Hub (LCD cooler, 6x QX120 fans)
- PrismRGB Prism S x2 (Strimer Plus ATX + GPU cables)
- Nollie 8 controller (8 channels of ARGB fans/strips)
- Razer Huntsman V2 (per-key RGB, 104 keys)
- Razer Basilisk V3 (11-zone scroll wheel + underglow)
- Razer Seiren V3 Chroma (ring light mic)
- Dygma Defy (split ergo, self-managed RGB)

**Room:**
- 3x WLED ESP32 controllers (desk underglow, monitor backlight, shelf accent)
- Philips Hue Bridge with 4 Play bars behind monitors
- Total: **12+ addressable RGB devices, 2000+ individual LEDs**

### Primary Use Cases

1. **Custom effect authoring** — Writes Lightscript effects in TypeScript with Three.js shaders. Designs audio-reactive visualizations that respond to music genre and energy. Has a library of 30+ custom effects.
2. **Unified orchestration** — All 12 devices running the same effect simultaneously, spatially mapped to their physical positions. The Strimer cables pulse in sync with the fan rings. The desk underglow follows the monitor backlight.
3. **Mood-based profiles** — `work` (warm amber at 20% brightness), `code` (SilkCircuit neon palette, subtle pulse), `gaming` (full reactive), `movie` (bias lighting only), `sleep` (all off with 2-minute fade).
4. **CLI-driven automation** — Switches profiles via shell aliases. `hc gaming` before launching Steam. `hc work` in her morning tmux script. Cron job fades to `sleep` at midnight.
5. **Debug and iterate** — Hot-reloads effects while developing. Needs real-time pixel preview, FFT spectrum overlay, per-device color output, frame timing metrics.

### Frustrations with Current Solutions

- **Proprietary RGB tools are Windows-only.** Full stop. She dual-boots just for RGB and it makes her want to throw things. An entire OS partition exists so her LEDs can dance.
- **Multiple conflicting daemons.** OpenRGB for mobo/GPU/RAM, OpenRazer for peripherals, OpenLinkHub for Corsair, and she still can't run a unified effect across all of them.
- **No CLI orchestration.** Every tool has a GUI. She doesn't want a GUI. She wants `hypercolor set aurora --speed 0.7 --palette silkcircuit`.
- **Effect development is painful.** The proprietary editor has no hot-reload, no debugger, no TypeScript support. She edits HTML in VS Code, alt-tabs to the app, clicks reload, waits, checks if it looks right. Repeat 400 times.
- **PrismRGB has zero Linux support.** Her Strimer cables and Nollie 8 controller -- the most visually dramatic parts of her build -- go dark on Linux. No open-source driver exists.

### Dream Features

- `hypercolor daemon` starts at boot via systemd, TUI available over SSH
- `hypercolor dev --watch effects/custom/aurora.ts` hot-reloads with live preview in terminal
- Spatial layout persists in TOML, version-controlled in her dotfiles
- D-Bus integration so Hyprland keybinds can trigger profile switches
- Frame timing overlay: render time, push latency per device, dropped frames
- Effect debugger: freeze frame, step forward, inspect per-LED color values
- Full Lightscript API compatibility so her existing effects just work

### Detailed Workflow: Creating a New Audio-Reactive Effect

```
1. Opens terminal (Ghostty), navigates to ~/dev/hypercolor/effects/custom/
2. Creates new file: `nebula-pulse.ts`
3. Scaffolds from template:
   $ hypercolor effect new nebula-pulse --template audio-reactive
   -> Creates nebula-pulse.ts with Lightscript boilerplate,
      meta tags for controls, audio API imports

4. Starts dev server:
   $ hypercolor dev --watch nebula-pulse.ts --preview tui
   -> Daemon hot-reloads the effect
   -> TUI shows live LED strip preview (true-color half-blocks)
   -> Audio spectrum overlay at bottom of terminal

5. Iterates on the shader code in Neovim (split pane)
   - Saves file -> effect hot-reloads in <100ms
   - TUI preview updates immediately
   - Adjusts FFT smoothing, bass threshold, color mapping

6. Tests with music:
   $ hypercolor capture audio --source pipewire --monitor
   -> FFT data flows into the effect
   -> She watches the TUI preview pulse with bass hits

7. Tweaks spatial mapping:
   $ hypercolor layout edit --zone "strimer-atx" --position 0.3,0.5 --rotation 15
   -> Adjusts where the Strimer cable samples from the canvas

8. Commits the effect to her dotfiles repo:
   $ hypercolor effect export nebula-pulse --to ~/dev/dotfiles/hypercolor/effects/

9. Deploys to production profile:
   $ hypercolor profile edit gaming --effect nebula-pulse --audio on
   $ hypercolor profile apply gaming
   -> All 12 devices light up. Bass drops. Room explodes with color.

10. Keybind setup (Hyprland):
    bind = SUPER SHIFT, L, exec, hypercolor profile cycle
    -> Cycles through profiles with a keyboard shortcut
```

### Edge Cases & Pain Points

- **Strimer cable orientation matters.** The ATX 24-pin is a 20x6 LED matrix. If it's mounted upside-down, the spatial mapping needs to flip vertically. She needs per-zone rotation/mirror controls.
- **Multi-monitor screen capture.** She runs triple 27" monitors. Screen ambience needs to capture all three and map to different zones (left monitor -> left Hue bars, right monitor -> right Hue bars).
- **SSH access.** Sometimes she SSHs into her desktop from her laptop. The TUI needs to work over SSH with no local GPU context. WebSocket proxy from daemon to remote TUI.
- **Effect unit testing.** She wants to write tests: "Given this FFT input, assert these LED colors." Deterministic render for CI.
- **Git conflict resolution.** Layout files are TOML. When she merges dotfile branches, TOML diffs should be human-readable and merge cleanly.

---

## Persona 2: Jake — The Plug-and-Play Gamer

### Profile

| | |
|---|---|
| **Name** | Jake |
| **Age** | 22 |
| **Occupation** | College student (Computer Science, junior year) |
| **Technical Skill** | Intermediate — comfortable with terminal basics, follows Arch Wiki when needed |
| **OS** | CachyOS (switched from Windows 11 last semester) |
| **Desktop** | KDE Plasma 6 |
| **Peripherals** | Corsair K70 RGB Pro, Logitech G502 X Plus, HyperX Cloud III |

### Hardware Setup

**PC (first build, budget-conscious):**
- MSI MAG B650 TOMAHAWK WIFI (RGB headers, Mystic Light)
- AMD Ryzen 7 7800X3D
- RTX 4060 Ti (minimal GPU RGB -- just a small logo strip)
- Corsair Vengeance RGB DDR5-5600 32GB (2x16GB)
- 3x Arctic P12 PWM PST A-RGB fans (daisy-chained to mobo header)
- 1x generic ARGB LED strip along the bottom of the case
- No WLED, no smart home devices

**Room:**
- Desk lamp (not smart)
- Monitor has no ambient lighting
- Dorm room -- roommate tolerance for RGB is moderate

### Primary Use Cases

1. **Set and forget.** Picks an effect that looks cool, applies it to everything, doesn't touch it for weeks. Currently rotates between rainbow wave, color cycle, and a static purple.
2. **Game-specific moods.** Wants different colors for different games. Blue for Valorant, red for Doom, green for Minecraft. Doesn't want to manually switch every time.
3. **Showing off.** When friends come over, he wants to pull up something impressive with one click. The "demo mode" that makes people say "whoa."
4. **Low effort.** Will spend 10 minutes max on initial setup. If it takes longer than that, he's going back to whatever default his mobo software set.

### Frustrations with Current Solutions

- **OpenRGB works but feels clinical.** He installed it, it detected his devices, he set a rainbow wave... and then what? The effects plugin has 50 effects but no previews, no thumbnails, no "this one is fire" curation. He doesn't know which ones are good.
- **No game integration.** On Windows, iCUE had profiles per game. OpenRGB doesn't know what game is running.
- **Conflicting software.** He installed OpenRGB AND OpenRazer AND Piper for his mouse. Something conflicts. His keyboard occasionally resets to hardware rainbow mode and he has no idea why.
- **Settings don't persist across reboots.** He sets up RGB, reboots, and it's back to the default MSI rainbow vomit. The systemd service for OpenRGB is janky.

### Dream Features

- **Gallery view.** Scrollable grid of effect thumbnails with animated previews. Click one. Done.
- **One-click apply to all.** "Apply this effect to every device" button. No per-device configuration unless he wants it.
- **Game detection.** Detects running game via Gamescope/Steam and switches profile automatically.
- **Persistence.** Just... works after reboot. Every time. Without a tutorial.
- **"Impress my friends" button.** A curated collection of the most visually striking effects with transitions between them.

### Detailed Workflow: First-Time Setup

```
1. Installs Hypercolor from AUR:
   $ paru -S hypercolor
   -> Installs daemon + CLI + web UI

2. Starts the daemon:
   $ sudo systemctl enable --now hypercolor
   -> Daemon starts, auto-discovers 6 devices:
      MSI mobo (OpenRGB), GPU (OpenRGB), RAM x2 (OpenRGB),
      Corsair K70 (OpenRGB), Logitech G502 (via hidapi)

3. Opens web UI in Firefox:
   -> http://localhost:9420
   -> Sees a welcome wizard

4. Welcome wizard flow:
   a. "We found 6 devices!" -- shows detected hardware with icons
   b. "Quick setup: pick a vibe"
      - Chill (slow gradients, warm tones)
      - Electric (fast, neon, reactive)
      - Stealth (minimal, single color accents)
      - Custom (skip to effect browser)
   c. Jake picks "Electric"
   d. All devices light up with a preset electric theme
   e. "Want audio-reactive? Allow microphone/system audio"
      - Jake clicks yes
      - Bass hits from Spotify make everything pulse
   f. "Save as startup profile?"
      - Yes -> persists across reboots

5. Browses effect gallery:
   -> Grid of 50+ effects with animated thumbnails
   -> Filters: Audio, Gradient, Gaming, Chill, Intense
   -> Clicks "Neon Rain" -- sees instant preview on his actual hardware
   -> Clicks "Apply" -- done

6. Sets up game profiles (optional, he finds this later):
   -> Settings > Game Integration > Enable
   -> Detects Steam library
   -> Assigns "Valorant" -> blue tactical theme
   -> Assigns "Cyberpunk 2077" -> neon pink/yellow

7. Never opens the app again for three weeks.
   -> It just runs. RGB stays set. Reboots work. Life is good.
```

### Edge Cases & Pain Points

- **Roommate complaints.** Needs a quick "dim everything to 20%" option without changing the effect. A global brightness slider accessible from system tray / D-Bus.
- **Laptop mode.** When he plugs his keyboard into his laptop for class, the keyboard should revert to a static color (or off). Device disconnect handling matters.
- **Firmware fights.** His Corsair K70 has onboard profiles. If OpenRGB and the keyboard firmware both try to control RGB, they fight. Need to properly take/release hardware control.
- **Doesn't read docs.** The web UI needs to be self-explanatory. Tooltips, not manpages.
- **Updates.** When Hypercolor updates, his profiles should not reset. Ever. This is the number one rage-quit trigger from his iCUE days.

---

## Persona 3: Luna — The Content Creator

### Profile

| | |
|---|---|
| **Name** | Luna |
| **Age** | 26 |
| **Occupation** | Full-time Twitch streamer & YouTube creator (3.2K avg viewers) |
| **Technical Skill** | Advanced user — not a developer, but configures complex OBS scenes, runs NGINX, manages her own streaming server |
| **OS** | Pop!_OS 24.04 (stable, NVIDIA just works) |
| **Desktop** | COSMIC |
| **Content** | Variety gaming, Just Chatting, art streams |

### Hardware Setup

**Streaming PC:**
- Intel i9-13900K, RTX 4080 SUPER
- Corsair Dominator Platinum RGB DDR5 64GB
- 3x Corsair LL120 RGB fans
- NZXT Kraken Z73 RGB AIO

**Peripherals:**
- Razer Huntsman V3 Pro (per-key RGB, analog switches)
- Razer DeathAdder V3 Pro (no RGB -- she values sensor over glow)
- Elgato Stream Deck XL (integrates with everything)
- GoXLR (audio mixer, has RGB ring)

**Room Lighting (the star of the show):**
- Elgato Key Light x2 (not RGB, but brightness-controlled)
- Philips Hue Bridge with 8 bulbs:
  - 2x Play bars behind main monitor
  - 2x Lightstrip Plus behind desk
  - 4x A19 color bulbs in room corners
- WLED ESP32 x3:
  - Triangular LED panels on back wall (camera background)
  - Under-desk strip
  - Shelf accent strip (behind figure collection)
- Govee Glide wall bars x3 (camera-visible, left wall)
- Nanoleaf Shapes hexagons x9 (camera-visible, right wall)

**Total: 20+ controllable light sources, mixing PC RGB with room lighting**

### Primary Use Cases

1. **Scene-based profiles.** Each OBS scene has a matching lighting profile:
   - `just-chatting`: warm pink/purple room, keyboard matches, calm vibes
   - `gaming-fps`: reactive blue/red, intense, alerts trigger flashes
   - `gaming-chill`: soft gradient, cozy, music-reactive at low intensity
   - `art-stream`: neutral white room lighting (accurate colors on camera), subtle desk underglow
   - `brb`: animated "be right back" pattern on wall panels, room dims to 30%
   - `starting-soon`: dramatic slow color sweep, builds anticipation
   - `raid-alert`: entire room flashes gold for 5 seconds, then returns to current scene
2. **Twitch integration.** Chat commands trigger lighting effects:
   - `!rainbow` -- 10-second rainbow wave across room (costs 500 channel points)
   - `!color red` -- shifts room accent to red for 30 seconds
   - `!rave` -- strobes for 5 seconds (rate-limited to prevent abuse)
   - `!mood chill` -- votes on room mood, majority wins after 60 seconds
3. **Transition effects.** When switching OBS scenes, lighting cross-fades over 1.5 seconds. No jarring jumps.
4. **Camera-aware lighting.** The wall panels and Hue bulbs are visible on camera. They need to look good on stream, not just in person. What looks cool in a room can look blown-out or flickery on a 1080p30 stream.

### Frustrations with Current Solutions

- **Nothing unifies PC and room lighting on Linux.** She currently runs OpenRGB for PC, the Hue app on her phone, WLED's web UI per device, and Govee's app. Switching a "scene" means touching 4 different interfaces.
- **No Twitch integration exists on Linux.** On Windows, her previous RGB tool had third-party Twitch plugins. On Linux? She wrote a Python script that calls OpenRGB's API when chat messages come in. It breaks constantly.
- **OBS integration is hacky.** She uses OBS websocket + a custom Node script to detect scene changes and trigger lighting. It's held together with duct tape and prayers.
- **Transition timing.** When she switches scenes, the lighting change lags behind the video transition by 200-500ms. It looks amateur. Her viewers notice.
- **Alert fatigue on hardware.** Raid alerts trigger a flash, but the flash command goes to OpenRGB (50ms), then WLED (80ms), then Hue (200ms). They're visibly out of sync.

### Dream Features

- **OBS Scene <-> Hypercolor Profile binding.** Scene "Just Chatting" auto-activates profile `just-chatting`. Instant. No scripts.
- **Twitch EventSub integration.** Native support for channel points, raids, subs, bits. Configurable effects per event type.
- **Cross-fade engine.** Smooth transitions between profiles. Configurable duration and easing curve.
- **Device group sync.** Mark devices as "synchronized" -- they receive color updates in the same frame, regardless of transport protocol latency differences. Buffer fast devices to wait for slow ones.
- **Stream Deck plugin.** Buttons that switch profiles, trigger one-shot effects, or show current profile name on the LCD.
- **"Camera preview" mode.** Shows what the lighting looks like through a webcam feed, so she can tune for on-camera appearance.

### Detailed Workflow: Going Live

```
1. 30 minutes before stream, Luna runs her pre-stream checklist:
   $ hypercolor profile apply starting-soon
   -> Room enters slow dramatic sweep
   -> PC RGB enters matching gradient
   -> Camera background panels do a gentle breathe animation

2. Opens OBS, selects "Starting Soon" scene
   -> Hypercolor detects OBS scene change via WebSocket
   -> Confirms lighting matches (already set)

3. Goes live. Chat fills in. After 5 minutes, switches to "Just Chatting" scene:
   -> OBS transition: 1.5s stinger
   -> Hypercolor cross-fades to just-chatting profile over 1.5s, synced to transition
   -> Room goes from dramatic sweep to warm pink/purple
   -> Keyboard shifts to soft magenta underglow
   -> Wall panels settle into static warm tones (camera-friendly)

4. Chat interaction:
   -> Viewer redeems "!rainbow" for 500 points
   -> Hypercolor triggers 10-second rainbow overlay on ALL devices
   -> After 10s, smoothly returns to just-chatting profile
   -> Viewer redeems "!color cyan"
   -> Room accent shifts to cyan for 30 seconds, then fades back

5. Switches to gaming (Valorant):
   -> OBS scene: "Gaming FPS"
   -> Hypercolor: gaming-fps profile
   -> Room goes dark except accent lighting (blue/red tactical)
   -> Keyboard enters per-key reactive mode (keys flash on press)
   -> Wall panels show a slow blue pulse
   -> Audio-reactive is ON: gunshots cause brief red flashes

6. Takes a break:
   -> OBS scene: "BRB"
   -> Hypercolor: brb profile
   -> Room dims to 30%
   -> Wall panels show animated "waves" pattern
   -> Desk underglow does a slow breathe

7. Someone raids with 200 viewers:
   -> Hypercolor triggers raid-alert effect:
      - ALL devices flash gold 3x over 3 seconds
      - Room ramps to full brightness momentarily
      - Keyboard does a gold wave sweep
   -> Returns to current profile after alert

8. Stream ends:
   -> OBS goes offline
   -> Hypercolor detects stream end, activates "post-stream" profile
   -> Room slowly fades to a calm purple over 30 seconds
   -> She doesn't have to touch anything
```

### Edge Cases & Pain Points

- **Hue bridge rate limits.** Hue Entertainment API supports ~25 updates/sec for 10 lights. At 60fps, Hypercolor needs to intelligently throttle Hue updates while keeping WLED/USB at full speed. Different update rates per transport.
- **Govee and Nanoleaf.** These devices have proprietary APIs. Govee has a cloud API (300ms latency, unusable for reactive). Nanoleaf has a local API (better). Supporting third-party smart lights is an ecosystem challenge.
- **Camera white balance.** When room lighting shifts color, her webcam auto-white-balance can compensate, making the color shift invisible on stream. She needs to lock WB and tune lighting to look right with locked settings.
- **Alert stacking.** If 3 raids happen in 10 seconds, the alert effects shouldn't stack and create a seizure-inducing lightshow. Queue and rate-limit.
- **Partner compliance.** Twitch partner agreements may restrict certain types of audience-triggered effects. The Twitch integration should have configurable safety limits.
- **Latency budget.** End-to-end from Twitch event to room lighting change must be under 500ms total. EventSub webhook -> Hypercolor API -> device push. Every millisecond counts for audience engagement.

---

## Persona 4: Marcus — The Smart Home Orchestrator

### Profile

| | |
|---|---|
| **Name** | Marcus |
| **Age** | 45 |
| **Occupation** | IT Infrastructure Manager at a mid-size company |
| **Technical Skill** | Expert in networking and systems, intermediate in development |
| **OS** | Ubuntu Server 24.04 LTS (headless Home Assistant host), Fedora 41 workstation |
| **Smart Home Hub** | Home Assistant OS on a dedicated Intel NUC |

### Hardware Setup

**Home Network:**
- UniFi Dream Machine Pro (manages VLAN segmentation for IoT)
- Dedicated IoT VLAN for all smart devices
- 2x UniFi AP U6 Pro (full house WiFi coverage)

**Lighting Inventory (30+ devices across 6 rooms):**

*Living Room:*
- Philips Hue Bridge #1: 6x A19 bulbs (ceiling fixtures), 2x Lightstrip Plus (TV backlight, bookshelf)
- WLED ESP32 #1: 150-LED strip along crown molding
- WLED ESP32 #2: 60-LED strip behind couch

*Kitchen:*
- Philips Hue: 4x A19 bulbs (recessed cans), 1x Lightstrip (under-cabinet)
- WLED ESP32 #3: 90-LED strip above cabinets

*Office:*
- PC: modest RGB (mobo header + RAM, controlled via OpenRGB)
- WLED ESP32 #4: monitor backlight (Hyperion-style)
- WLED ESP32 #5: desk underglow
- Hue: 2x Play bars behind monitors

*Master Bedroom:*
- Hue: 2x A19 (nightstand lamps), 1x Lightstrip (headboard)
- WLED ESP32 #6: ceiling cove strip (100 LEDs)

*Bathroom:*
- WLED ESP32 #7: mirror backlight (40 LEDs, waterproof)

*Porch/Exterior:*
- WLED ESP32 #8: 300-LED strip along roofline (seasonal colors)
- Hue: 2x outdoor Lily spots

**Total: 32 controllable devices, ~1500 addressable LEDs, 2 Hue bridges, 8 WLED controllers**

### Primary Use Cases

1. **Circadian lighting.** Living room and bedroom follow a daily color temperature curve: 6500K bright blue-white at noon, gradually warming to 2700K amber by evening, dimming to 2200K nightlight by 10pm. Aligned with his family's sleep schedule.
2. **Occupancy-based scenes.** Motion sensors trigger room-specific profiles. Walk into the kitchen at 7am? Lights come on at 4000K 80%. Walk in at 11pm? 2200K 20% -- just enough to find water without waking anyone.
3. **Room-by-room control.** Each room operates independently. Wife is reading in the bedroom (warm, dim) while he's gaming in the office (blue, reactive) while the kids are watching a movie in the living room (TV ambient, everything else off).
4. **Holiday/seasonal automation.** Exterior roofline goes red/green for December, orange/purple for October, red/white/blue for July 4th. Automated, no ladder climbing.
5. **Away mode.** When the family leaves (detected via UniFi device presence), lights cycle through a realistic "someone is home" pattern. Randomized room activation, realistic timing.
6. **"All off" panic button.** One HA dashboard button turns off every light in the house. Essential when the kids discover they can yell "Hey Google, party mode."

### Frustrations with Current Solutions

- **Home Assistant WLED integration is good but limited.** It controls on/off, brightness, effect selection, and color. But it can't push arbitrary per-LED color data. For synchronized multi-room effects, he needs something that speaks DDP directly.
- **Hue + WLED don't talk to each other.** Matching a Hue bulb color to a WLED strip requires manual color picking or complex HA automations with template sensors. There's no "make these two things look the same."
- **No unified spatial awareness.** His crown molding strip and ceiling recessed lights occupy the same visual space. There's no tool that understands "these 6 lights are all in the living room ceiling" and can run a gradient across them as a unified zone.
- **OpenRGB + HA integration is nonexistent.** His office PC RGB is an island. No HA entity, no automation. He can't even turn off his PC RGB when he leaves the room without a custom script.
- **Reliability is paramount.** His wife's threshold for "smart home bullshit" is exactly zero. If the bedroom lights don't respond at 6am because some daemon crashed, he's in trouble. Current setup has too many moving parts.
- **Update anxiety.** Every HA update risks breaking a WLED or Hue integration. He runs HA on a 3-month delay and still gets burned.

### Dream Features

- **Room abstraction.** Define rooms with heterogeneous devices (Hue + WLED + OpenRGB). Apply a single color/effect to the room and Hypercolor translates to each device's protocol.
- **Circadian engine.** Built-in circadian rhythm support. Configure wake time, sleep time, latitude. Hypercolor computes color temperature curves and applies them across all warm-white-capable devices.
- **Home Assistant integration.** Hypercolor exposes itself as HA entities. Each room is a light entity with brightness, color, and effect controls. Full automation support.
- **Scheduling.** Built-in scheduler for recurring profiles. "Weekdays 6am: bedroom gentle wake. Weekends: let us sleep."
- **Watchdog.** Self-healing daemon. If a WLED device goes offline, Hypercolor retries reconnection every 30 seconds. If the daemon crashes, systemd restarts it. Alert via HA notification if a device has been offline for 10 minutes.
- **WAF (Wife Acceptance Factor) dashboard.** A simple, non-technical web UI page: room buttons with preset scenes. "Living Room: Movie Night." Tap. Done. No sliders, no hex codes, no effect browsers.

### Detailed Workflow: Configuring a New Room

```
1. Marcus just installed a WLED strip behind his new standing desk in the office.
   The ESP32 is on his IoT VLAN at 10.10.20.45.

2. Opens Hypercolor web UI from his workstation:
   -> http://hypercolor.local:9420 (mDNS)
   -> Dashboard shows all rooms and devices

3. Navigates to Devices > Discover:
   -> Hypercolor scans mDNS for WLED devices
   -> "WLED-desk-strip (10.10.20.45) - 72 LEDs" appears
   -> Clicks "Add to Room"
   -> Selects "Office" from room dropdown (or creates new room)

4. Spatial configuration:
   -> Office room view shows existing devices:
      - "Monitor backlight" (WLED, 60 LEDs) -- positioned behind monitors
      - "Desk underglow" (WLED, 45 LEDs) -- under desk edge
      - "Hue Play L" and "Hue Play R" -- behind monitors
      - "PC RGB" (OpenRGB) -- inside case
   -> Drags the new strip to the back edge of the desk area
   -> Sets topology: Strip, 72 LEDs, horizontal orientation

5. Assigns to room profile:
   -> Office has three profiles: "Work", "Gaming", "Meeting"
   -> Opens "Work" profile:
      - Color temperature: 5000K
      - Brightness: 70%
      - Effect: Static
   -> The new desk strip inherits the room profile automatically

6. Tests:
   -> Clicks "Preview" -- all office devices show the Work profile
   -> New strip lights up at 5000K 70% in sync with existing devices
   -> Switches to "Gaming" -- all devices transition to blue/purple reactive

7. Automation rules:
   -> Room Automation > Office:
      - Trigger: Motion sensor (HA entity) detects presence
      - Action: If 6am-6pm -> "Work" profile. If 6pm-midnight -> "Gaming" profile.
      - If no motion for 15 minutes -> fade to off over 60 seconds
   -> Saves. The rule is stored in Hypercolor's config, but the motion trigger
      comes from HA via the integration.

8. Verifies reliability:
   -> Checks device health: all green
   -> Enables watchdog alerts for the new device
   -> Sets a 30-second reconnect interval if it goes offline
   -> Adds to his Uptime Kuma monitoring (via Hypercolor's /health endpoint)
```

### Edge Cases & Pain Points

- **Cross-VLAN communication.** WLED devices are on IoT VLAN (10.10.20.x), Hypercolor daemon runs on his workstation or server VLAN (10.10.10.x). Needs mDNS relay or static configuration. Firewall rules for DDP (UDP port 4048) and WLED JSON API (TCP port 80).
- **Hue Bridge rate limits.** Entertainment API is fast but limited to 10 lights per entertainment zone. His living room has 8 Hue devices. If he wants per-bulb control at 25fps, he's right at the limit.
- **Mixed-capability devices.** Hue bulbs support color temperature natively. WLED strips are RGB only -- "3000K warm white" has to be approximated as an RGB value. The circadian engine needs to understand device capabilities.
- **Family members.** His 14-year-old discovered WLED's web UI and keeps setting the living room to rave mode. Hypercolor needs access control or at least an "override lock."
- **Power consumption.** 1500 LEDs at full white draws significant power. His UPS reports jump 80W when everything is on. He needs a global power budget or at least visibility into per-room estimated wattage.
- **Firmware updates.** 8 WLED controllers need periodic OTA updates. It would be helpful if Hypercolor could report firmware versions and flag when updates are available, but never auto-update without confirmation.

---

## Persona 5: Yuki — The Aesthetic Alchemist

### Profile

| | |
|---|---|
| **Name** | Yuki |
| **Age** | 19 |
| **Occupation** | Art school student (digital illustration + motion graphics) |
| **Technical Skill** | Beginner-intermediate -- comfortable with creative software, not with config files |
| **OS** | Fedora 41 with GNOME (switched for free creative tools: Krita, Blender, DaVinci Resolve) |
| **Creative Tools** | Krita, Blender, DaVinci Resolve, Figma (browser) |
| **Socials** | Instagram (@yuki.palettes), TikTok, Behance |

### Hardware Setup

**PC (clean, minimal, curated):**
- White NZXT H7 Flow case
- No excessive RGB -- just what came with the build:
  - Corsair Vengeance RGB DDR5 32GB (subtle top-edge RGB)
  - 1x Phanteks DRGB strip inside case (hidden, edge-lit glow)
  - Keychron Q1 HE (per-key RGB, QMK/VIA)

**The Real Setup (room aesthetic):**
- WLED ESP32 x2:
  - Behind desk: 120-LED strip, diffused behind frosted acrylic panel
  - Behind headboard: 60-LED strip
- Govee Glide hexagons x7 on the wall above the desk (camera visible for TikToks)
- Philips Hue: 2x A19 in desk lamps (one each side)
- IKEA DIRIGERA hub with 3x TRADFRI color bulbs (ceiling track)

**Total: ~11 devices, modest LED count but extremely intentional placement**

### Primary Use Cases

1. **Color palette curation.** Every month, Yuki creates a new color palette for their art. They want their room lighting to match. This month it's `#1a1a2e`, `#16213e`, `#0f3460`, `#e94560` -- a moody cyberpunk palette. The desk glow should be `#0f3460`, the wall hexagons should alternate between `#e94560` and `#16213e`, the headboard should be a gradient.
2. **Precise color control.** Not "blue" -- `#0f3460`. Yuki works in hex, HSL, and OKLCH. They need a color picker that speaks their language and renders accurately on LED hardware (as close as possible given RGB LED gamut limitations).
3. **Gradient design.** The desk strip isn't one color -- it's a 5-stop gradient. `#1a1a2e` on the left, through `#0f3460` in the center, to `#e94560` on the right. This gradient needs to be designable, not just "pick start and end color."
4. **Palette import.** They build palettes on Coolors.co, Adobe Color, and in Figma. They want to paste a Coolors URL or import an ASE/JSON palette file and have it map to their room.
5. **Instagram content.** Their room aesthetic is part of their personal brand. Lighting changes are TikTok content. They record "palette makeover" videos showing the room transitioning between color schemes.
6. **Mood boards.** They design in terms of mood: "rainy day," "golden hour," "neon alley." Each mood maps to a saved profile with specific colors, brightness, and optional subtle animation (slow pulse, gentle gradient shift).

### Frustrations with Current Solutions

- **WLED's color picker is a joke.** HSV wheel with no hex input on mobile. Entering precise hex values requires the desktop UI which is clunky. No palette management at all.
- **Every device has a different color space.** She sets `#e94560` on WLED and the same hex on Hue and they look completely different. Hue is warmer, WLED is more saturated. No color calibration exists anywhere in the ecosystem.
- **No gradient support.** Every app treats LED strips as a single color. She wants gradients, and the only way to get them is through WLED effects (which are pre-programmed and not customizable to specific hex values).
- **Palette workflows are manual.** She designs a palette in Coolors, writes down the hex codes, manually enters them one by one into each device's app, adjusts because they look wrong, gives up, opens TikTok.
- **Animation is all-or-nothing.** Effects are either "static" or "rave party." She wants subtle: a 30-second gradient shift by 2 degrees of hue. A 10-minute slow pulse between two colors. Gentle, organic, alive.

### Dream Features

- **Professional color picker.** Hex, HSL, OKLCH, RGB input. Color wheel with lightness slider. Saved palettes. Recent colors. Color harmony suggestions (complementary, triadic, analogous).
- **Palette import.** Paste a Coolors URL, upload an ASE file, paste a CSS custom property block, import from Figma plugin. Palette appears in Hypercolor. One-click apply to room.
- **Gradient editor.** Multi-stop gradient designer. Drag stops on a strip representation. Preview on actual hardware in real-time. Save gradients as presets.
- **Color accuracy mode.** Basic per-device color calibration. "This WLED strip renders reds warm -- shift red channel -5%." Not ICC-profile level, but enough to get devices visually close.
- **Subtle animation controls.** Instead of predefined effects: "breathe between these two colors over 15 seconds." "Shift hue +10 degrees over 5 minutes, then reverse." Parametric, organic motion.
- **Shareable profiles.** Export a profile as a URL. Other Hypercolor users can import it. Yuki posts profiles on their Instagram: "link in bio for my cyberpunk palette."

### Detailed Workflow: Monthly Palette Refresh

```
1. Yuki finishes their latest illustration series -- "neon rain" theme.
   The palette is saved in Coolors: coolors.co/palette/1a1a2e-16213e-0f3460-533483-e94560

2. Opens Hypercolor web UI on their phone (responsive design matters):
   -> Taps Palettes > Import
   -> Pastes the Coolors URL
   -> Hypercolor fetches the palette: shows 5 swatches with hex values
   -> Names it "Neon Rain" and saves

3. Opens the Room Designer (simplified spatial view):
   -> Sees their room layout with device positions
   -> Selects "Desk Strip" (120 LEDs)
   -> Taps "Apply Gradient"
   -> Gradient editor appears:
      - Drag palette colors onto gradient stops
      - Left: #1a1a2e (deep navy)
      - Center-left: #16213e (dark blue)
      - Center: #0f3460 (medium blue)
      - Center-right: #533483 (purple)
      - Right: #e94560 (hot pink)
   -> Live preview shows the gradient on the actual strip

4. Configures wall hexagons:
   -> Selects Govee hexagon group
   -> "Alternate" mode: odd panels get #533483, even panels get #e94560
   -> Adds subtle animation: 20-second cross-fade between the two colors

5. Configures ambient:
   -> Hue desk lamps: #0f3460 at 40% brightness
   -> IKEA ceiling: #1a1a2e at 25% (barely visible, just ambiance)
   -> Headboard strip: static #16213e at 30%

6. Saves as profile "Neon Rain":
   -> Preview shows the whole room in the new palette
   -> Taps Save

7. Records TikTok:
   -> Sets up phone on tripod aimed at desk area
   -> Starts recording
   -> Hypercolor: "Transition from current to Neon Rain over 5 seconds"
   -> Room sweeps from old palette to new
   -> Captures the moment
   -> Posts with #roomaesthetic #desksetup #rgblighting

8. Shares profile:
   -> Palettes > Neon Rain > Share
   -> Gets a link: hypercolor.app/p/yuki-neon-rain (or JSON export)
   -> Posts to Instagram bio
```

### Edge Cases & Pain Points

- **LED color gamut limitations.** RGB LEDs physically cannot reproduce some colors. Very dark, desaturated tones like `#1a1a2e` may appear as "off" or "barely visible blue" on WS2812 LEDs. Hypercolor needs to show a "this is what it will actually look like" preview that accounts for minimum brightness thresholds.
- **Govee integration.** Govee's local API is undocumented and reverse-engineered. Cloud API adds 300ms+ latency and requires internet. If Hypercolor supports Govee, it needs to handle the fragility of unofficial APIs gracefully.
- **Phone-first experience.** Yuki primarily uses their phone for Hypercolor. The web UI must be fully responsive and touch-friendly. Gradient editor with drag-and-drop on mobile is a UX challenge.
- **IKEA DIRIGERA.** IKEA's smart home hub uses Matter/Thread. Hypercolor would need Matter support to control TRADFRI bulbs natively, or go through Home Assistant as a bridge.
- **Color space math.** Converting between hex (sRGB), OKLCH (perceptual), and actual LED output requires awareness of gamma, color temperature mixing, and per-strip calibration. This is a rabbit hole. "Good enough" is acceptable; "pixel perfect" is impossible.
- **Social sharing.** If profiles become shareable, Hypercolor needs to handle the case where someone imports a profile designed for a 120-LED desk strip onto their 30-LED strip. Graceful scaling.

---

## Persona 6: Kai — The Plugin Developer

### Profile

| | |
|---|---|
| **Name** | Kai |
| **Age** | 31 |
| **Occupation** | Senior Software Developer at a cloud infrastructure company, open-source contributor |
| **Technical Skill** | Expert -- contributes to Rust OSS projects, maintains two crates on crates.io |
| **OS** | NixOS (of course) |
| **Languages** | Rust, TypeScript, Go |
| **OSS Contributions** | OpenRGB (submitted a controller driver), WLED (minor PRs), various Rust crates |

### Hardware Setup

**Dev/Test Rig:**
- Govee H6167 LED strip (their target device -- WiFi, proprietary protocol)
- Govee Glide hexa panels (same protocol family)
- WLED ESP32 (reference device for comparison testing)
- USB logic analyzer (for protocol debugging)
- Spare ESP32s (for WLED firmware development)
- A breadboard with WS2812B LEDs for raw testing

**Personal PC:**
- Modest build, not a gamer
- Only RGB: 2x Corsair Vengeance RGB sticks (barely notices them)
- WLED strip behind monitor (his one aesthetic indulgence)

### Primary Use Cases

1. **Building a Govee backend.** Govee devices use a proprietary UDP protocol on port 4003 (local API) with a cloud fallback. Kai is reverse-engineering the local protocol and building a Hypercolor device backend so Govee users don't have to go through the cloud.
2. **Plugin development workflow.** He wants to write Rust code that implements `DeviceBackend`, compile it, and test it against real hardware -- all without modifying Hypercolor's core. Ideally as a Wasm plugin or at minimum a gRPC bridge process.
3. **Debug logging and tracing.** When packets arrive at the Govee strip malformed, he needs to see exactly what was sent -- hex dump, timing, retry logic. Structured tracing with device-specific spans.
4. **Device simulation.** For CI and development without hardware, he needs a virtual device that accepts the same protocol and displays the "received" colors in a terminal or web view.
5. **Automated testing.** His backend should have integration tests that start a mock Govee device, push 1000 frames, and verify color accuracy, timing, and error handling.
6. **Documentation.** He'll write API docs for the device backend trait, contribute a "plugin developer guide," and document the Govee protocol for future contributors.

### Frustrations with Current Solutions

- **OpenRGB's plugin system is C++ and Qt.** He submitted a controller once. The build system took 3 hours to set up. Qt dependency management is a nightmare on NixOS. The contribution experience was painful despite good documentation.
- **No device simulator.** When developing the OpenRGB controller, he had to have the physical device plugged in for every test. Couldn't run CI because the CI runner didn't have a Govee strip attached.
- **Protocol debugging is primitive.** He's using Wireshark for UDP capture and a custom Python script to decode packets. There's no integrated view of "what Hypercolor sent" vs "what the device received."
- **Wasm plugin story is immature everywhere.** He's excited about Hypercolor's planned Wasm plugin system but wary of WIT interface stability. He's been burned by WASI preview changes before.
- **No development-mode daemon.** He wants to run Hypercolor in a mode where only his plugin is active, verbose logging is on, and the render loop sends a test pattern instead of requiring an actual effect.

### Dream Features

- **`DeviceBackend` trait with excellent documentation.** Every method, every error case, every lifecycle hook documented with examples. `cargo doc` should be a joy to read.
- **Plugin dev CLI.** `hypercolor plugin new govee --type device` scaffolds a plugin project with the trait implemented, a mock device, and a test harness.
- **Device simulator framework.** `HypercolorSim` -- a virtual device that implements the network protocol (or USB HID) and displays received colors. Ships with simulators for WLED/DDP, Hue, and HID. Plugin devs can extend it for their protocol.
- **Structured tracing integration.** `tracing` spans per device, per frame, per packet. Filter to just Govee traffic: `RUST_LOG=hypercolor::device::govee=trace`.
- **Hot-reload for plugins.** Change the plugin code, recompile, and the daemon picks it up without restart. For Wasm plugins, this is watch + reload. For gRPC bridge, it's process restart.
- **Plugin CI template.** GitHub Actions workflow that runs the plugin's tests against the simulator. No hardware required.

### Detailed Workflow: Building the Govee Backend

```
1. Kai forks hypercolor on GitHub, clones locally.

2. Scaffolds the plugin:
   $ hypercolor plugin new govee --type device --transport udp
   -> Creates crates/hypercolor-govee/ with:
      - Cargo.toml (depends on hypercolor-core for traits)
      - src/lib.rs (DeviceBackend trait implemented with TODOs)
      - src/protocol.rs (empty, for Govee protocol encoding)
      - src/discovery.rs (mDNS discovery stub)
      - tests/integration.rs (test harness with mock device)
      - README.md with protocol notes template

3. Starts the device simulator:
   $ hypercolor sim --device govee --port 4003 --leds 30
   -> Opens a virtual Govee device on UDP 4003
   -> TUI shows 30 "virtual LEDs" as colored blocks
   -> Accepts Govee protocol packets and renders colors

4. Implements the protocol:
   - Reads Govee protocol docs (captured via Wireshark + the Govee app)
   - Implements `discover()` -- mDNS scan for _govee._udp
   - Implements `connect()` -- sends status query, gets device info
   - Implements `push_frame()` -- encodes colors as Govee UDP packets

5. Tests against the simulator:
   $ cargo test -p hypercolor-govee
   -> Integration tests start the mock device
   -> Push 100 frames of rainbow colors
   -> Verify timing, color accuracy, packet format
   -> All pass

6. Tests against real hardware:
   $ hypercolor daemon --dev --plugin govee --only-device "Govee Strip"
   -> Daemon starts in dev mode
   -> Only the Govee backend is active
   -> Sends test pattern (rainbow sweep) to the real strip
   -> Kai watches the strip, checks colors, verifies timing

7. Debug session:
   $ RUST_LOG=hypercolor::device::govee=trace hypercolor daemon --dev --plugin govee
   -> Sees every packet in structured log:
      2026-03-01T20:15:03.123Z TRACE govee::push_frame:
        device_id="H6167-A1B2C3" frame=42 packet_size=138
        hex=01 03 01 00 1E FF0000 00FF00 0000FF ...
   -> Spots a byte-order bug in the color encoding
   -> Fixes, recompiles, hot-reloads

8. Submits PR:
   -> PR includes: backend implementation, tests, simulator extension,
      protocol documentation, CI workflow
   -> CI runs tests against the simulator automatically
   -> Reviewers can test without owning a Govee device
```

### Edge Cases & Pain Points

- **Govee protocol fragmentation.** Different Govee models use different packet formats. H6167 uses one format, H615B uses another, Glide uses a third. The backend needs a per-model protocol handler with a device database.
- **Discovery reliability.** mDNS on Linux can be flaky, especially across VLANs. Some Govee devices only respond to broadcast discovery on their local subnet. Kai needs fallback to manual IP entry.
- **Cloud API rate limits.** If the local protocol fails, the fallback cloud API has a rate limit of 10 requests/minute per device. That's 0.17fps. Completely useless for real-time effects. The backend should warn users and refuse to use cloud API for real-time modes.
- **Wasm plugin memory limits.** If the plugin runs as Wasm, it has limited memory and no direct network access. All UDP communication has to go through the WIT host interface. This adds complexity to protocol implementation.
- **Plugin versioning.** When Hypercolor updates the `DeviceBackend` trait, existing plugins break. Semantic versioning and trait versioning strategy matters from day one.
- **NixOS build environment.** Hypercolor's Servo dependency requires specific Rust toolchain versions and system libraries. Kai needs a `flake.nix` that actually works. This is non-trivial.

---

## Persona 7: Sam — The Audio-Reactive Producer

### Profile

| | |
|---|---|
| **Name** | Sam |
| **Age** | 35 |
| **Occupation** | Music producer (electronic / synthwave) and live performer |
| **Technical Skill** | Advanced user -- runs a complex DAW setup, comfortable with MIDI routing, latency optimization, and audio programming |
| **OS** | Ubuntu Studio 24.04 (low-latency kernel, JACK audio) |
| **DAW** | Bitwig Studio 5 |
| **Audio** | Focusrite Scarlett 18i20 (3rd gen), JACK + PipeWire bridge |

### Hardware Setup

**Studio:**
- PC: Ryzen 9 7950X, 128GB RAM, RTX 4070 (not a gaming rig -- GPU for GPU-accelerated plugins)
- Minimal PC RGB (doesn't care about case lighting)
- WLED ESP32 x4:
  - Behind 3x 27" monitors (total: 180 LEDs, screen-ambience style)
  - Under desk (90 LEDs, downward-facing, creates floor glow)
  - Ceiling panels: 2x custom-built LED matrix panels (20x10 each = 400 LEDs total)
  - Behind rack gear (60 LEDs, vertical strip)
- Philips Hue: 4x Play bars at corners of room (accent wash)
- Nanoleaf Canvas squares x12 on the wall behind him (visible during live streams)

**MIDI Controllers:**
- Akai APC40 Mk2 (grid of RGB buttons -- not Hypercolor controlled, but MIDI input source)
- Novation Launch Control XL (knobs for live parameter tweaking)
- Custom Arduino MIDI controller (4x rotary encoders assigned to Hypercolor controls)

**Total: ~750 LEDs across 8 zones, plus MIDI integration**

### Primary Use Cases

1. **Audio-reactive studio lighting.** Every surface reacts to the music. Bass hits pulse the floor glow. Mids animate the wall panels. Highs sparkle the ceiling. The room IS the visualizer.
2. **Beat-locked effects.** Not just "react to audio" -- locked to the BPM. At 128 BPM, the color cycle completes exactly every 4 beats. Phase-aligned with the DAW's transport clock. Mathematically precise.
3. **MIDI control.** Rotary encoder on his custom controller maps to effect parameters: knob 1 = bass sensitivity, knob 2 = color palette rotation, knob 3 = effect intensity, knob 4 = global brightness. Real-time, tactile, zero latency.
4. **Live performance mode.** During live DJ sets, he triggers lighting cues from his APC40. Pad 1 = build-up (ramp brightness and speed). Pad 2 = drop (full blast, freeze color on bass). Pad 3 = breakdown (dim, slow gradient). Pad 4 = kill all (blackout).
5. **DAW sync.** Bitwig sends MIDI clock. Hypercolor locks to it. When the DAW is stopped, lights freeze. When it plays, they animate. Tempo changes are followed instantly. Transport sync, not just BPM detection.
6. **Low latency above all.** Audio-to-light latency must be imperceptible. Under 10ms from audio peak to LED color change for USB devices. He'll tolerate 30ms for WLED (UDP travel time) but expects the processing pipeline itself to add less than 5ms.

### Frustrations with Current Solutions

- **LedFx is close but not there.** Audio-reactive, web UI, Python backend. But: no MIDI integration, no beat-lock (only beat detection, which drifts), no DAW sync, and it only outputs to WLED. His Hue lights and Nanoleaf are separate ecosystems.
- **WLED Sound Reactive firmware has latency.** The ESP32 runs its own FFT, which means the audio signal goes: room -> microphone on ESP32 -> FFT on ESP32 -> LED output. The microphone picks up room acoustics, reflections, and noise. Direct audio feed from the DAW is cleaner.
- **No MIDI input in any RGB tool.** Not LedFx, not OpenRGB, not Artemis, not anything on Linux. He wrote a custom Python bridge using `python-rtmidi` to translate MIDI CC to OpenRGB API calls. It works but latency is 40ms.
- **Beat detection vs. beat sync.** Every audio-reactive tool uses onset detection, which detects beats after they happen (5-20ms delay by definition). He wants BPM sync from the DAW's clock, which knows beats before they happen. Zero-latency beat alignment.
- **FFT resolution trade-offs.** Most tools use 512-sample FFT windows. That's fine for bass but terrible for harmonic analysis. He wants configurable FFT sizes: 2048 for harmonic/chord detection, 256 for transient/onset detection, running in parallel.

### Dream Features

- **MIDI input source.** MIDI CC, notes, and clock as first-class input sources. Map any MIDI event to any effect parameter. MIDI learn: click a control in the UI, twist a knob, done.
- **MIDI clock sync.** Lock the render loop to incoming MIDI clock (24 PPQ). Beat phase is derived, not detected. When Bitwig is at beat 3.5 of a 4-beat pattern, Hypercolor knows.
- **Configurable FFT pipeline.** Multiple FFT window sizes running simultaneously. 256-sample for onset detection (1.3ms resolution at 192kHz), 2048-sample for harmonic analysis. User-selectable smoothing, attack, decay per band.
- **Spectral features as effect inputs.** Not just bass/mid/treble. Spectral centroid, spectral flux, chromagram, dominant pitch, chord mood (minor/major), onset confidence -- all available as uniforms in effects (the full Lightscript audio API).
- **Direct audio input.** Capture from JACK/PipeWire, not from a microphone. Select the specific DAW output port. Zero acoustic interference.
- **Performance monitoring.** Frame timing histogram. "Audio capture: 0.3ms, FFT: 0.4ms, Effect render: 1.2ms, Device push: 0.8ms, Total: 2.7ms." If any stage exceeds budget, visual warning.
- **OSC support.** Open Sound Control for integration with Ableton, TouchDesigner, Max/MSP. OSC is the lingua franca of creative tech. If Hypercolor speaks OSC, it plugs into the entire audiovisual production ecosystem.

### Detailed Workflow: Studio Session

```
1. Sam opens Bitwig Studio, loads his latest synthwave track.

2. Starts Hypercolor daemon with MIDI + audio inputs:
   $ hypercolor daemon \
       --audio-source "Focusrite USB:Loopback" \
       --midi-clock "Bitwig:Clock" \
       --midi-cc "Arduino MIDI:Control"
   -> Daemon captures audio directly from DAW loopback
   -> MIDI clock is received but transport is stopped -- lights are frozen
   -> MIDI CC from Arduino is mapped and waiting

3. Loads his "Studio Session" profile:
   $ hypercolor profile apply studio-session
   -> Effect: "spectrum-cathedral" (custom audio-reactive shader)
   -> Mapping: bass -> floor, mids -> walls, treble -> ceiling, harmonic hue -> Nanoleaf

4. Presses Play in Bitwig:
   -> MIDI clock starts
   -> Hypercolor locks to 128 BPM, beat phase 0
   -> Lights begin animating in sync with the beat
   -> Bass synth hits: floor glows deep purple on every downbeat
   -> Hi-hats: ceiling sparkles on every 16th note
   -> Pad chords: wall panels shift hue based on chord mood
      (C minor -> cold blue, G major -> warm amber)

5. Tweaks parameters live:
   -> Rotary 1 (bass sensitivity): increases from 0.5 to 0.8
      -- floor pulse becomes more dramatic
   -> Rotary 2 (palette rotation): shifts palette 45 degrees
      -- overall color scheme rotates from purple/blue to blue/cyan
   -> Rotary 3 (intensity): pushes to 1.0
      -- everything goes brighter, more contrast between dark and light moments
   -> All changes are smooth (slew-limited to avoid jumps)

6. Build-up section:
   -> Music builds with a rising filter sweep
   -> Spectral centroid rises -> Hypercolor detects this as a "build"
   -> Auto-behavior: brightness gradually increases, speed increases
   -> Sam's foot taps

7. The drop:
   -> Massive bass hit at beat 1
   -> Floor and desk underglow flash full white for 1 frame
   -> Hue bars strobe once (as fast as the Entertainment API allows)
   -> Then settles into heavy bass-locked pulse
   -> The entire room throbs with the kick drum

8. Session ends. Sam stops Bitwig transport:
   -> MIDI clock stops
   -> Lights freeze on current color (not black, not default -- FREEZE)
   -> He reviews the session: frame timing logs show 2.1ms average latency
   -> No dropped frames across the 45-minute session

9. Exports the lighting "performance" as data:
   -> $ hypercolor capture export --format csv --output ~/session-lights.csv
   -> CSV of per-LED color values at 60fps for the entire session
   -> He'll use this in TouchDesigner for a music video
```

### Edge Cases & Pain Points

- **JACK vs. PipeWire.** Ubuntu Studio uses JACK for low-latency audio. PipeWire is the future but not every pro-audio user has migrated. Hypercolor needs to support both, and handle the JACK+PipeWire bridge gracefully. `cpal` abstracts some of this, but JACK-specific features (port connections, xrun handling) may need direct integration.
- **MIDI clock jitter.** USB MIDI has timing jitter of 1-3ms. Over a long session, this accumulates. Hypercolor needs a PLL (phase-locked loop) that smooths incoming MIDI clock and maintains phase coherence. Abrupt tempo changes should be detected and handled differently from gradual drift.
- **Multi-rate output.** WLED at 60fps, Hue at 25fps, Nanoleaf at 30fps. The beat-locked effect needs to render at the highest rate and downsample for slower devices, while maintaining beat phase alignment on all outputs.
- **Audio buffer sizes.** Sam runs Bitwig at 128 samples (2.7ms at 48kHz) for low latency. Hypercolor's audio capture buffer should be independent -- it can use larger buffers for FFT without affecting DAW latency. But it must read from the same audio graph without adding xruns.
- **Performance recording.** Exporting 60fps color data for 750 LEDs over 45 minutes = ~120MB of CSV. Needs streaming export, not in-memory buffering.
- **Intellectual property.** His custom effects and performance data are part of his artistic output. Hypercolor should never phone home, and exported data formats should be open and documented.

---

## Persona 8: Robin — The Windows Refugee

### Profile

| | |
|---|---|
| **Name** | Robin |
| **Age** | 40 |
| **Occupation** | Graphic designer at a print shop, hobbyist PC builder |
| **Technical Skill** | Intermediate -- built PCs for 15 years, used Linux casually, not a developer |
| **Former OS** | Windows 11 (switched after the Recall controversy) |
| **Current OS** | Linux Mint 22 (wanted something familiar) |
| **Former RGB Software** | Proprietary RGB software (paid subscription, 2 years) |

### Hardware Setup

**PC (enthusiast build, not bleeding-edge):**
- ASUS TUF Gaming Z790 (AURA Sync RGB)
- Intel i7-13700K
- EVGA GeForce RTX 3080 FTW3 (RGB on the shroud)
- G.Skill Trident Z5 RGB DDR5 48GB (2x24GB)
- Corsair iCUE H150i Elite LCD (AIO with LCD screen + 3x RGB fans)
- 6x Corsair LL120 RGB fans
- Lian Li Strimer Plus V2 (ATX 24-pin + GPU 8-pin)
- 2x generic ARGB strips (connected to mobo headers)

**Peripherals:**
- SteelSeries Apex Pro (per-key RGB)
- SteelSeries Rival 650 (2-zone RGB)
- No room lighting -- RGB lives inside the case

### What Robin Had on Windows

**Previous Setup (years of customization):**
- 4 saved profiles: "Daily Driver" (subtle blue gradient), "Gaming" (reactive red/orange), "Movie" (dim ambient), "Off" (hardware standby colors)
- Spatial layout meticulously configured: every device positioned to match physical location in the case
- Custom effect parameters tuned: speed = 3.7, wave height = 0.6, palette = "Ocean Dusk"
- The Strimer cables were the pride of the setup -- rainbow wave perfectly synchronized with the fan ring pattern
- Everything unified under one app. One click, entire case responds.

### Primary Use Cases

1. **Recreate the Windows experience.** Robin wants exactly what they had. Same effects, same colors, same spatial layout. They didn't switch to Linux because they wanted change -- they switched because Windows forced their hand.
2. **Minimal ongoing maintenance.** Set it up once, forget it exists. Check on it maybe once a month when something catches their eye and they want to tweak a color.
3. **Unified control.** The thing they loved most about their previous tool was ONE app for EVERYTHING. Not three tools. Not five browser tabs. One.
4. **Import, not recreate.** They have 2 years of configuration data in their previous tool. Layouts, profiles, effect parameters. They don't want to start from scratch.

### Frustrations with Current Solutions (the Linux transition)

- **Their previous Windows-only tool doesn't run on Linux. At all.** Wine? Nope. The USB HID drivers are Windows kernel-level. This was the hardest part of leaving Windows -- Robin genuinely considered keeping a Windows partition just for RGB control.
- **OpenRGB is confusing.** Installed it, it found some devices, but the Effects Plugin is a separate download, the Visual Map Plugin is another download, the UI looks like it was designed in 2005, and the Strimer cables aren't supported at all.
- **Lost their spatial layout.** Years of careful positioning -- "the ATX Strimer is here, the GPU Strimer is there, fan 3 is at the bottom-right" -- all gone. The proprietary layout export format is a JSON blob that nothing else reads.
- **Effect parity is poor.** The effects that look stunning in proprietary tools (3D wave, neon rain, aurora borealis) have no equivalent in OpenRGB's plugin. The plugin has "rainbow" and "breathing" and... that's about the quality level.
- **Corsair LCD display is orphaned.** The AIO's LCD screen showed CPU temp and a GIF on Windows. On Linux, it's blank. OpenLinkHub can drive it, but it's yet another separate tool.
- **Strimer cables are dark.** Prism S controllers have zero Linux support. The most visually dramatic part of Robin's build is completely non-functional.

### Dream Features

- **Migration wizard.** "Import your setup" button. Point it at the previous config directory. It imports:
  - Device layout (with automatic mapping to Hypercolor device IDs)
  - Effect assignments (with closest-match mapping for effects that have equivalents)
  - Color palettes and custom parameter values
  - Profile names and hotkey assignments
- **HTML effect compatibility.** Run the actual HTML effect files from the existing effect library. Hypercolor's Servo renderer should handle them natively since they're just Canvas 2D / WebGL pages.
- **Familiar UI.** A web interface that feels approachable. Not a terminal. Not a config file. A graphical tool with drag-and-drop, preview, and "Apply" buttons. Robin knows what they want their setup to look like -- the tool should make achieving it obvious.
- **Strimer support on day one.** This is make-or-break. If Hypercolor can drive Prism S controllers (which have reverse-engineered protocols documented in DRIVERS.md), it instantly becomes the only Linux tool that can control the full setup.
- **"It looks the same" validation.** A way to compare "what it looked like before" vs "what it looks like on Hypercolor." Side-by-side preview, ideally.

### Detailed Workflow: Migrating from Windows

```
1. Robin still has Windows on another drive. Boots into Windows one last time.

2. Exports previous RGB data:
   -> Previous tool > Settings > Export Configuration
   -> Saves to USB drive: rgb-export.zip
   -> Contains: layouts.json, profiles.json, effects/ folder

3. Boots into Linux Mint. Installs Hypercolor:
   -> Downloads .deb from hypercolor.dev
   -> $ sudo dpkg -i hypercolor_0.1.0_amd64.deb
   -> $ sudo systemctl enable --now hypercolor

4. Opens web UI:
   -> http://localhost:9420
   -> First-time setup detects devices:
      - ASUS TUF Z790 (OpenRGB) -- detected
      - EVGA RTX 3080 (OpenRGB) -- detected
      - G.Skill RAM x2 (OpenRGB) -- detected
      - Corsair H150i + fans (OpenLinkHub bridge) -- detected
      - SteelSeries keyboard + mouse (OpenRGB) -- detected
      - Prism S Strimers x2 (Hypercolor native HID) -- DETECTED!
      - ARGB strips (mobo header, OpenRGB) -- detected
   -> "11 of 11 devices detected!" -- Robin's eyes widen

5. Migration wizard:
   -> "Migrating from Windows? Import your setup!"
   -> Uploads rgb-export.zip
   -> Wizard processes:
      a. Device matching: maps source device IDs to Hypercolor devices
         - "ASUS AURA Controller" -> "ASUS TUF Z790 AURA"
         - "Prism S #1" -> "PrismRGB Prism S (16D0:1294) #1"
         - Shows mapping table, lets Robin confirm/adjust
      b. Layout import: recreates spatial positions
         - Shows before (original screenshot) and after (Hypercolor preview)
         - Minor position adjustments needed (different canvas aspect ratio)
      c. Effect mapping:
         - "Neon Rain" -> exact match (same HTML file runs on Hypercolor's Servo renderer)
         - "Spectral Wave" -> exact match
         - "Screen Ambience" -> mapped to Hypercolor's screen capture mode
         - "iCUE Temperature" -> mapped to hardware-sensor effect
      d. Profile import:
         - "Daily Driver", "Gaming", "Movie", "Off" -- all recreated
         - Effect parameters preserved (speed = 3.7, wave height = 0.6)

6. Robin clicks "Apply Daily Driver":
   -> All 11 devices light up
   -> The Strimers are alive again (for the first time on Linux!)
   -> The subtle blue gradient sweeps across the case
   -> It looks... the same. It looks the same as Windows.
   -> Robin exhales.

7. Minor tweaks:
   -> The GPU Strimer orientation was flipped (mounted differently since case mod)
   -> Opens layout editor, selects GPU Strimer, rotates 180 degrees
   -> Effect now flows in the correct direction

8. Sets as startup profile, closes the browser.
   Doesn't open Hypercolor again for 3 weeks.
   Everything just works.
```

### Edge Cases & Pain Points

- **Import format compatibility.** The export format from previous tools is undocumented and may change between versions. The migration wizard needs to handle multiple format versions and degrade gracefully when fields are missing.
- **Effect compatibility is not 100%.** Some effects use proprietary APIs (`device.setImage()` for LCD, etc.). These won't work. The wizard should clearly indicate which effects are compatible, which need adjustment, and which are incompatible.
- **Corsair LCD content.** On Windows, the iCUE LCD showed animated content (GIFs, sensor data). Hypercolor would need OpenLinkHub integration and a way to push rendered content to the LCD. This is a separate subsystem.
- **EVGA is defunct.** Robin's RTX 3080 FTW3 is an EVGA card. EVGA left the GPU market. OpenRGB support for EVGA cards may not be maintained long-term. Hypercolor should handle "device detected but no longer maintained" gracefully.
- **Robin is not technical.** They won't debug driver issues, read tracing output, or file GitHub issues with reproduction steps. The UX must be self-healing or at minimum provide actionable error messages: "Your Strimer cable isn't responding. Try: 1. Unplug and replug the USB cable. 2. Check that your user is in the 'plugdev' group."
- **Dual-boot coexistence.** Robin may keep Windows around for a while. If they boot Windows, iCUE and other RGB tools will reconfigure hardware RGB settings. When they boot back to Linux, Hypercolor needs to re-assert control without getting confused by stale device state.

---

## Day-in-the-Life Scenarios

### Bliss: The Full Spectrum Day

**06:30 — Wake-Up**
Hypercolor's circadian schedule activates `morning` profile. The WLED strip behind the headboard (she added one, inspired by Marcus's setup) fades from off to 2700K warm white over 10 minutes. PC is in sleep mode -- case lighting is in hardware standby (PrismRGB Prism 8 runs its built-in breathing effect at 5% brightness, configured via Hypercolor's shutdown handler).

**07:15 — Coffee & Code**
She logs into her desktop. Hypercolor daemon is already running (systemd service). Profile auto-switches to `work` based on time-of-day rule. All RGB goes warm amber at 20% brightness. Subtle. Functional. The Razer keyboard backlight illuminates only the keys she touches frequently (custom per-key map). Desk WLED strip: 3000K, diffused.

**09:00 — Deep Work**
She triggers `focus` via keybind (Super+Shift+F). All RGB dims to 10%. Only the keyboard backlight remains functional. Desk lamp is her primary light source. The room doesn't distract.

**12:00 — Lunch Break**
She puts on a synthwave playlist. Manually activates `vibe` profile from the TUI (she's already in terminal): `hypercolor profile apply vibe`. Audio-reactive kicks in. Case RGB does a slow spectrum shift. Desk strip pulses gently with bass. Strimer cables cycle through SilkCircuit neon palette. She eats a sandwich and watches the lights dance.

**14:00 — Hypercolor Development**
She's working on a new effect for Hypercolor itself. Opens `hypercolor dev --watch effects/custom/silk-cascade.ts --preview tui`. Split terminal: Neovim on left, TUI preview on right. She tweaks shader uniforms, saves, watches the preview update in <100ms. Adjusts the chromagram color mapping. Saves again. The SilkCircuit palette shimmers across 30 virtual LEDs in the TUI. Satisfied.

**18:30 — Gaming**
Launches Steam. The D-Bus integration detects Gamescope activating for Elden Ring. Profile switches to `gaming`. Everything goes reactive: audio-driven, high intensity, warm fire palette. Keyboard per-key effects react to keypresses. The room comes alive.

**21:00 — Movie Night**
Activates `movie` profile. All case RGB dims to near-zero. Only the WLED monitor backlight remains active in screen-ambience mode, capturing the display colors and projecting them onto the wall behind the monitors. Hue Play bars match. The room becomes the screen.

**23:30 — Sleep**
Cron job fires: `hypercolor profile apply sleep`. Everything fades to black over 2 minutes. Hardware standby colors kick in for devices that support it (a very dim breathing on the Corsair AIO ring, the rest off). Daemon continues running for remote access.

---

### Jake: The Low-Effort Day

**08:00 — Class**
PC is off. Jake is in a lecture. His Corsair keyboard is in his backpack (he brings it to class because "the laptop keyboard sucks"). When plugged into his laptop, the keyboard has no Hypercolor daemon -- it runs its onboard profile (static blue, set once via VIA).

**15:00 — Back at Dorm**
Plugs keyboard back into desktop, boots PC. CachyOS loads, Hypercolor daemon starts automatically. His `default` profile (set during initial wizard 3 weeks ago) activates: "Electric" preset with neon cyan/purple theme. Everything lights up. He doesn't think about it.

**16:00 — Valorant**
Opens Steam, launches Valorant via Gamescope. Hypercolor detects the game and switches to his `Valorant` profile (blue tactical theme, audio-reactive gunshot flashes). He set this up once during the initial wizard and has never touched it since.

**19:00 — Friends Come Over**
His buddy Ethan says "yo, make it do the thing." Jake opens Hypercolor's web UI on his phone (he bookmarked it), taps "Gallery," scrolls to "Showcase Mode." It cycles through the 5 most visually dramatic effects with 15-second transitions. Ethan: "that's sick." Mission accomplished.

**22:00 — Roommate Complaint**
His roommate Tyler asks him to dim the lights. Jake taps the system tray icon (GNOME extension), slides the global brightness to 15%. The effect keeps running but dimmer. Tyler can sleep. Crisis averted.

**23:30 — Sleep**
Jake just... closes his laptop lid and walks away. PC stays on. RGB stays on at 15% brightness. He'll deal with it tomorrow. (Eventually he'll find the auto-sleep setting. Eventually.)

---

### Marcus: The Orchestrated Home

**06:00 — Wake-Up Automation**
Home Assistant triggers `morning` scene. The bedroom Hue bulbs fade from off to 2700K over 15 minutes, synchronized with his wife's alarm. Bedroom WLED strip stays off (she doesn't like it in the morning). Hypercolor processes the HA scene change and adjusts its managed devices accordingly.

**06:30 — Kitchen**
Marcus walks into the kitchen. Motion sensor triggers. HA sends event to Hypercolor. Kitchen Hue recessed cans come on at 4000K 80%. Under-cabinet WLED strip activates at warm white. Time-aware: this is the weekday morning profile (brighter than weekend).

**08:00 — Office**
He sits down at his desk. Presence detection (via HA + room presence sensor) activates `office-work` profile. Monitor backlight WLED comes on at neutral white. Desk underglow is a subtle warm amber. Hue Play bars match the desk underglow. OpenRGB sets the PC RAM and mobo to a static cool blue at low brightness. Functional, not distracting.

**12:00 — Lunch**
He walks away from the office. 15-minute absence timer fires. Office lights fade to off over 60 seconds. Energy saved.

**17:00 — Family Time**
Living room occupancy detected (multiple phones on WiFi). HA triggers `living-room-evening`. Hue ceiling bulbs warm to 3000K. Crown molding WLED strip does a slow warm gradient. TV backlight WLED is in standby (will activate when TV turns on via CEC detection).

**18:30 — Movie Night**
TV turns on. HA detects CEC signal, sends event. Hypercolor activates `movie` profile for the living room: ceiling Hue dims to 10%, crown molding WLED dims to 5%, TV backlight WLED enters screen-ambience mode (captures TV HDMI via capture card and projects ambient colors). Couch WLED does a very subtle warm breathe.

**21:00 — Kids to Bed**
HA scene: `nighttime`. Bedroom lights for kids' rooms (not managed by Hypercolor -- separate Hue bridge) dim. Living room transitions to `evening-quiet`: slightly warmer, slightly dimmer. Crown molding WLED shifts to 2200K equivalent.

**22:30 — House Goes Dark**
HA schedule triggers `goodnight`. Every Hypercolor-managed device fades to off over 2 minutes. Exterior roofline WLED shifts to a very dim warm white (porch light equivalent, 5% brightness). Marcus's phone shows the Hypercolor dashboard: all rooms green, no offline devices. He plugs in and goes to sleep.

**03:00 — Bathroom Trip**
Bathroom motion sensor fires. WLED mirror backlight comes on at 2200K, 10% brightness. Just enough to navigate without blinding. Turns off 5 minutes after motion stops.

---

## Frustration Map

### What Makes Users Angry About Current RGB Software

#### 1. Platform Lock-In (Severity: Critical)

> *"I built a $3000 PC and I can't change my LED colors because I'm running the wrong operating system."*

- iCUE, Aura Sync, SteelSeries GG, Razer Synapse -- all Windows-only
- Users who want Linux have to choose between their OS and their RGB
- Some maintain a Windows partition/VM literally just for lighting control
- The entire enthusiast RGB ecosystem ignores 4% of desktop users entirely
- **Who feels this most:** Bliss, Robin, Kai

#### 2. Software Fragmentation (Severity: High)

> *"I have six apps running just to make my lights work, and they all fight each other."*

- Corsair devices need iCUE. Razer needs Synapse. ASUS needs Armoury Crate.
- OpenRGB unifies some, but not all (no Corsair iCUE LINK, no PrismRGB)
- Each app has its own effect engine, its own profiles, its own system tray icon
- They conflict: iCUE and OpenRGB both try to control the mobo AURA header
- RAM usage: iCUE (300MB) + Synapse (200MB) + Armoury Crate (150MB) = 650MB for lights
- **Who feels this most:** Robin, Marcus, Luna

#### 3. Effects That Lie (Severity: Medium)

> *"The preview showed a beautiful aurora. My case looks like a clown vomited."*

- Effect thumbnails/previews are rendered on a flat 2D canvas
- Actual LED output depends on spatial mapping, LED density, diffusion, brightness
- A smooth gradient on screen becomes 6 discrete color blocks on a fan ring
- High-frequency effects (fine patterns, small details) are invisible on sparse LED layouts
- **Who feels this most:** Yuki, Robin, Jake

#### 4. Devices That Vanish (Severity: High)

> *"My RAM shows up as 'Unknown Device' after every reboot. It was working yesterday."*

- USB device enumeration order changes between boots
- I2C/SMBus detection is fragile (race conditions with kernel modules)
- WLED devices go offline when WiFi drops momentarily
- Hue Bridge occasionally fails to respond to mDNS queries
- OpenRGB detection is a one-time scan -- no retry, no reconnection
- **Who feels this most:** Marcus, Robin, Sam

#### 5. Settings Amnesia (Severity: Critical)

> *"I spent an hour getting everything perfect. Updated the software. Everything reset."*

- iCUE updates occasionally wipe profiles (Corsair's most-reported bug)
- OpenRGB doesn't auto-apply profiles on startup without manual service configuration
- Firmware updates on devices can reset hardware lighting modes
- Config files are stored in undocumented locations, not backed up
- No version control, no export/import, no cloud sync
- **Who feels this most:** Robin, Jake, Marcus

#### 6. Performance Bloat (Severity: Medium)

> *"My RGB software uses more RAM than the game I'm playing."*

- iCUE: 250-400MB resident memory
- Razer Synapse: 150-250MB + Razer Central + Razer Cortex
- Proprietary tools: 200-500MB (Chromium renderer for effects)
- Armoury Crate: 150MB + multiple background services
- Combined, RGB software can consume 1GB+ and multiple CPU cores
- Gamers notice: 2-5 FPS drop attributed to background RGB services
- **Who feels this most:** Jake, Sam (latency-sensitive), Kai (principle)

#### 7. No Automation (Severity: Medium)

> *"I want my lights to change when I start a game, leave the room, or play music. Why can't they?"*

- Most RGB software is manual: open app, pick effect, close app
- Game integration exists in some tools (iCUE, proprietary alternatives) but is Windows-only
- Time-based scheduling requires third-party tools or scripts
- Presence detection, audio reactivity, smart home integration -- all DIY
- **Who feels this most:** Marcus, Luna, Sam

#### 8. Closed Ecosystems (Severity: High)

> *"I reverse-engineered this protocol by sniffing USB packets for two weeks. Why is this information not public?"*

- Corsair, Razer, ASUS, SteelSeries -- all proprietary protocols
- No official SDKs for hardware control (or SDKs that are Windows-only, like Corsair's)
- Community reverse-engineering is the only path (OpenRGB, OpenRazer)
- The LightScript effect format is documented but device plugins are JavaScript without specs
- Plugin development requires reading other people's minified code
- **Who feels this most:** Kai, Bliss

---

## Delight Moments

### What Makes Users Fall in Love with RGB

#### 1. "The Bass Drop Moment"

The first time the music hits and the entire room responds -- desk, case, wall, ceiling -- all pulsing together with the bass. It's not lighting anymore, it's synesthesia. Sam lives for this moment. Even Jake, who "doesn't care about audio reactive," has this moment when a friend plays music and says "wait, it DOES that?"

**How Hypercolor captures this:** Zero-config audio reactive mode. Plays a demo track during onboarding with the user's actual hardware. The first beat drop happens within 60 seconds of setup.

#### 2. "The Setup Tour"

Someone walks into the room, sees the lighting, and says "what IS this?" Robin lives for the moment guests notice the Strimer cables pulsing in sync with the fan rings. Luna's viewers clip her room lighting transitions. Jake's friend Ethan asks "how do I get this?"

**How Hypercolor captures this:** Shareable profiles with QR codes. "Scan this to see my setup." One-click demo mode for when friends are over.

#### 3. "It Just Works"

After weeks/months of fighting software, something finally works reliably. Every boot, every device, every time. No troubleshooting, no re-detection, no profile re-application. Robin's transition moment: "I opened my PC for the first time in a week and everything was exactly how I left it."

**How Hypercolor captures this:** Reliability as a feature. Daemon watchdog. Automatic reconnection. Profile persistence that survives updates. Boot-time service with sub-3-second device initialization.

#### 4. "The Perfect Palette"

Yuki spends 45 minutes designing a color palette. They apply it to their room. They look around. The desk glow is exactly `#0f3460`. The wall panels alternate perfectly. The headboard strip gradients smoothly. They take a photo. It looks even better than they imagined.

**How Hypercolor captures this:** Professional color tools. Gradient editor. Live preview on real hardware. Color accuracy calibration. "What you see is what you get" -- no more "that looked different on screen."

#### 5. "The Transition"

Smooth, choreographed transitions between scenes. Luna switching from "Just Chatting" to "Gaming" -- the room cross-fades over 1.5 seconds, perfectly timed to her OBS stinger transition. No jarring cuts, no flicker, no lag. It looks professional.

**How Hypercolor captures this:** Built-in cross-fade engine. Configurable easing curves. OBS integration for transition sync. Sub-frame timing accuracy.

#### 6. "The Discovery"

Finding a new effect that perfectly matches a mood. Scrolling through the gallery, clicking one at random, and the room transforms. "Oh. This one. This is the one." The moment where technology disappears and it's just... vibes.

**How Hypercolor captures this:** Curated effect gallery with animated previews. One-click apply with instant rollback. "You might also like" recommendations based on current preferences.

#### 7. "The First Custom Effect"

Bliss writes 20 lines of TypeScript. Saves the file. Watches the TUI preview update. Tweaks a uniform. The colors shift. Tweaks again. Pushes it to all devices. Sees her custom creation running on 2000 LEDs across 12 devices. Something she built, running on her hardware, looking beautiful.

**How Hypercolor captures this:** First-class developer experience. Hot-reload in under 100ms. Effect templates. Debug preview. The barrier from "idea" to "running on hardware" should be measured in minutes.

#### 8. "The Silent Room"

It's midnight. The automation dims everything to almost-off. Just a faint warm glow from the desk strip. The PC hums quietly. The room feels calm. The lighting didn't just look cool -- it changed how the room feels. Marcus realizes his sleep has improved since the circadian lighting. Sam notices he's more creative in the amber-lit studio.

**How Hypercolor captures this:** Circadian lighting isn't a power-user feature. It's a default. Gentle fades. Warm nights. The software cares about wellbeing, not just spectacle.

---

## Accessibility Personas

### A1: Devon — Color Vision Deficiency

| | |
|---|---|
| **Name** | Devon |
| **Age** | 30 |
| **Condition** | Deuteranopia (red-green color blindness, affects ~8% of males) |
| **Severity** | Moderate -- can see some reds/greens but distinguishes them poorly |
| **Technical Skill** | Intermediate -- casual gamer, moderate Linux user |

**The Challenge:** Devon uses a color-centric application but can't reliably distinguish between red and green -- two of the three RGB channels. The app's UI might use red/green for status indicators. The effect gallery shows thumbnails that look identical to him. And when he's choosing colors for his setup, he can't tell if his room is "warm amber" or "sickly green."

**What Hypercolor Must Do:**
- **UI:** Never rely solely on red/green to convey information. Use shapes, labels, and patterns alongside color. Device status: green checkmark becomes a labeled "Online" badge. Error states use red + an icon + text.
- **Color Picker:** Include a CVD simulation preview. "This is how this palette looks with deuteranopia." Toggle between normal vision and simulated. Offer curated palettes that are CVD-friendly (blues, yellows, purples -- the safe zone).
- **Effect Gallery:** Thumbnails should include the effect name prominently, not rely on color alone to differentiate. Category labels matter more than color-coded borders.
- **Accessibility setting:** Global CVD mode that adjusts the entire UI's color scheme to be distinguishable. Not a color filter -- a genuinely redesigned palette.

**Workflow Consideration:** Devon wants to impress friends with his setup, just like Jake. But when he picks colors, he needs confidence that "this looks right." A preview mode that says "this effect is safe for CVD viewers" would help him choose without second-guessing.

### A2: Morgan — Photosensitivity

| | |
|---|---|
| **Name** | Morgan |
| **Age** | 25 |
| **Condition** | Photosensitive epilepsy (diagnosed at 16) |
| **Triggers** | Flashing lights >3Hz, high-contrast strobing, rapid red/blue alternation |
| **Technical Skill** | Intermediate -- uses Linux for programming coursework |

**The Challenge:** RGB lighting effects are a minefield. Strobe effects, rapid color cycling, beat-reactive flashing, and high-contrast transitions can trigger seizures. Morgan loves ambient lighting but is terrified of accidentally activating a dangerous effect. Most RGB software has zero awareness of photosensitivity risk.

**What Hypercolor Must Do:**
- **Safety filter (mandatory):** All effects are analyzed for photosensitivity risk before playback. Flash frequency, contrast ratio, and color transition speed are evaluated. Effects exceeding WCAG 2.3 flash thresholds (3 flashes/second, or any flash where the area is large enough and bright enough) are flagged.
- **Three-tier safety system:**
  - "Safe" -- guaranteed no rapid flashing, gentle transitions only
  - "Moderate" -- may contain moderate-speed color changes, no strobing
  - "Unchecked" -- has not been analyzed or exceeds safe thresholds
- **Global safety lock:** Morgan can enable "Photosensitivity Safe Mode" in settings. This:
  - Hides all "Unchecked" effects from the gallery
  - Enforces a maximum transition speed on all effects (no change faster than 3Hz)
  - Disables strobe, flash, and rapid-cycle parameters in effect controls
  - Limits maximum brightness delta per frame
- **Twitch integration safety:** For Luna's stream, audience-triggered effects must respect the streamer's (or viewer's) safety settings. `!rave` should be blockable per-user.

**Workflow Consideration:** Morgan wants to browse effects without fear. The gallery must clearly label safety tiers. Previews should play at the effect's actual speed, not sped up for impact. A "preview in safe mode" option lets Morgan see what the effect looks like with safety constraints applied.

### A3: Priya — Motor Impairment

| | |
|---|---|
| **Name** | Priya |
| **Age** | 37 |
| **Condition** | Repetitive strain injury (RSI) in both hands, uses ergonomic split keyboard + trackball |
| **Limitations** | Cannot perform precise drag-and-drop. Fine cursor control is painful. Prolonged mouse use causes flare-ups. |
| **Technical Skill** | Advanced -- senior developer, heavy keyboard user |

**The Challenge:** The spatial layout editor is a drag-and-drop interface. The gradient editor requires dragging color stops. The color picker requires fine cursor movement. All of these are painful or impossible for Priya. She can use a keyboard fluently and can do imprecise clicking, but anything requiring sustained mouse precision is a barrier.

**What Hypercolor Must Do:**
- **Full keyboard navigation:** Every feature accessible via keyboard. Tab through devices in the spatial editor. Arrow keys to reposition. Enter to confirm. No feature should require a mouse.
- **Keyboard-driven spatial editor:** Select a device zone with Tab, then use arrow keys (with configurable step size) to position it. Numeric input for exact coordinates: "Position X: 0.35, Y: 0.72."
- **Keyboard-driven gradient editor:** Tab between color stops. Enter to edit color. Arrow keys to reposition stop. Add/remove stops with keybinds, not tiny click targets.
- **Large click targets:** Buttons, sliders, and interactive elements should be at minimum 44x44px (WCAG touch target size). Slider handles should be grabbable with imprecise clicks.
- **Reduce repetitive actions:** Batch operations ("apply this color to all fans"), templates ("use the same layout as last time"), and presets reduce the number of individual interactions needed.
- **CLI as the great equalizer:** `hypercolor layout set-position "strimer-atx" 0.35 0.72` achieves in one command what the GUI requires 30 seconds of dragging. The CLI isn't just for power users -- it's an accessibility tool.

**Workflow Consideration:** Priya configures her setup once and rarely changes it. The initial setup experience is the critical moment. If she can get through first-time configuration without a flare-up, she's a happy user. Templates, presets, and keyboard-first design make this possible.

---

## Persona Priority Matrix

### Who do we build for first?

| Persona | Priority | Rationale |
|---|---|---|
| **Bliss** | P0 (Alpha) | She's building it. Dogfooding drives quality. If the developer experience is excellent, everything else follows. |
| **Robin** | P0 (Alpha) | Migration story is the growth engine. Every Windows refugee who successfully migrates becomes an advocate. Strimer support is the killer feature. |
| **Jake** | P1 (Beta) | Volume user. The onboarding wizard and effect gallery must exist before Hypercolor can grow beyond developers and power users. |
| **Marcus** | P1 (Beta) | Smart home integration is the feature that no competitor has on Linux. HA + WLED + Hue unified control is a unique selling point. |
| **Luna** | P2 (v1.0) | Twitch/OBS integration is valuable but depends on a stable core. Build on the event bus architecture. |
| **Yuki** | P2 (v1.0) | Color tools and gradient editor are design polish. The core must work before it's beautiful. |
| **Sam** | P2 (v1.0) | Audio-reactive is foundational (Phase 0), but MIDI sync, OSC, and DAW integration are advanced features. |
| **Kai** | P3 (v1.x) | Plugin ecosystem requires a stable API. Wasm plugins are Phase 4 in the roadmap. Build the API right in Phase 0-1, open it in Phase 4. |

### Feature Mapping by Persona

| Feature | Bliss | Jake | Luna | Marcus | Yuki | Kai | Sam | Robin |
|---|---|---|---|---|---|---|---|---|
| CLI orchestration | **Primary** | - | - | Secondary | - | Secondary | Secondary | - |
| Web UI | Secondary | **Primary** | **Primary** | **Primary** | **Primary** | Secondary | Secondary | **Primary** |
| TUI | **Primary** | - | - | - | - | Secondary | - | - |
| Effect gallery | - | **Primary** | Secondary | - | **Primary** | - | - | **Primary** |
| Spatial layout editor | **Primary** | Secondary | Secondary | **Primary** | Secondary | - | Secondary | **Primary** |
| Audio reactive | Secondary | Secondary | Secondary | - | - | - | **Primary** | - |
| MIDI input | - | - | - | - | - | - | **Primary** | - |
| Twitch integration | - | - | **Primary** | - | - | - | - | - |
| OBS integration | - | - | **Primary** | - | - | - | - | - |
| HA integration | - | - | Secondary | **Primary** | - | - | - | - |
| Game detection | Secondary | **Primary** | Secondary | - | - | - | - | - |
| Migration wizard | - | - | - | - | - | - | - | **Primary** |
| Plugin API/SDK | Secondary | - | - | - | - | **Primary** | - | - |
| Color picker/palette | - | - | - | - | **Primary** | - | - | - |
| Circadian lighting | - | - | - | **Primary** | - | - | - | - |
| Device simulator | - | - | - | - | - | **Primary** | - | - |
| Safety/accessibility | - | - | - | - | - | - | - | - |

**Safety and accessibility are horizontal concerns -- they apply across all features, not to any single persona.**

---

## Design Principles Derived from Personas

1. **Zero-to-wow in under 5 minutes.** Jake and Robin prove that if the first experience isn't magical, users leave. The onboarding wizard must detect devices, apply a stunning preset, and save as persistent in under 5 minutes.

2. **CLI and GUI are equal citizens.** Bliss lives in the terminal. Robin lives in the browser. Neither is the "real" interface. Both must be complete. Priya depends on the CLI for accessibility.

3. **Profiles are sacred.** Settings must survive updates, reboots, dual-boots, and device reconnections. Robin's migration story proves that years of configuration are emotionally valuable. Treat profiles like user data, not app state.

4. **Automation is the multiplier.** Marcus and Luna show that manual control doesn't scale. Time-based, presence-based, content-based, and event-based automation transforms lighting from a toy into infrastructure.

5. **Color accuracy matters.** Yuki's workflow reveals that "close enough" isn't. The gap between displayed hex values and actual LED output is a trust problem. Acknowledge the gap, minimize it, and never pretend it doesn't exist.

6. **Safety is non-negotiable.** Morgan cannot use an app that might trigger a seizure. Photosensitivity filtering isn't a nice-to-have -- it's a legal and ethical requirement. Default to safe; opt in to intensity.

7. **The plugin ecosystem is the moat.** Kai represents the contributors who will make Hypercolor support 100 devices instead of 12. The API must be stable, documented, and joyful to develop against. Win the developers, win the ecosystem.

8. **Latency is a feature.** Sam's 5ms budget teaches us that performance isn't an optimization -- it's a user requirement. Audio-to-LED latency, profile switch speed, hot-reload time, device push latency: measure everything, budget everything, regress nothing.

---

*These personas are living documents. As Hypercolor's user base grows, revisit and refine them. Add real user interview data when available. The personas should evolve from "informed speculation" to "grounded in research" as the project matures.*
