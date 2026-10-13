---
name: effect-livecoding
description: >-
  Write, debug, and tune a Hypercolor lighting effect against a running daemon in a tight
  loop: install an HTML page, apply it once, read its script errors with line numbers in
  your own file, look at the rendered canvas, patch controls live, and hot swap new
  versions without re-applying. Use this whenever someone asks you to create, write,
  design, port, fix, or iterate on a Hypercolor effect, an RGB or LED animation, or a
  lighting visual; when an effect "looks wrong", "is black", "froze", "only fills part of
  the strip", or "ignores its slider"; or when you need to see what Hypercolor is
  rendering right now. Works through the MCP tools (install_effect, capture_canvas,
  get_effect_console, adjust_controls) or the `hypercolor` CLI (effects install,
  capture, console, patch); it never needs the source tree.
---

# Effect Livecoding

You can see what you render and read what it throws, so never declare an effect done
from the source alone. Every change ends with a console read and a capture.

## The loop

| Step | MCP | CLI |
| --- | --- | --- |
| 1. Install the page | `install_effect {html, file_name}` | `hypercolor effects install fx.html` |
| 2. Apply it, once | `set_effect {query: <id>}` | add `--activate` to the install |
| 3. Read errors | `get_effect_console {after: <console_after>, level: "warn"}` | `hypercolor effects console --after <N> --level warn` |
| 4. Look at it | `capture_canvas {frames: 4, interval_ms: 200}` | `hypercolor effects capture -o fx.png --frames 4 --interval-ms 200`, then view the files |
| 5. Tune live | `adjust_controls {zone, layer, values}` | `hypercolor effects patch --param speed=2` |
| 6. Edit and reinstall | `install_effect` with the same `file_name` | `hypercolor effects install fx.html` |

Both install paths return `console_after`, a console cursor taken just before the
page loads. Pass it to step 3 so you read only what this version printed. Give the
page about a second to load and throw before you read.

Without MCP or the CLI, the REST equivalents are `POST /api/v1/effects/install`
(multipart field `file`), `GET /api/v1/effects/console?after=N&level=warn`, and
`GET /api/v1/canvas/frame?format=png&width=320`.

## How reloads behave

- **Keep one file name for the whole session.** The effect id comes from the
  installed path, so reinstalling the same name replaces the effect in place and
  a layer already running it reloads the new page on its own. A new name makes a
  second effect that nothing is running.
- **Do not re-apply after a reinstall.** `set_effect` and `--activate` replace the
  zone's whole layer stack and reset every control to its default.
- **Every reinstall restarts the page.** Time, particles, and any other script
  state start over. Use controls for tuning, since a control patch reaches the
  running page without a reload.
- **Editing the installed file in place also hot reloads,** through a file watcher
  on the user effects directory (the `path` the install returned). It skips
  validation, so prefer installing. A directory that did not exist when the
  daemon started is not watched until it restarts.

## Reading what you get back

- **The capture is the canvas, not the LEDs.** It shows the composed image at
  full brightness before the spatial sampler maps it onto LED positions. Detail
  finer than the LED spacing disappears on hardware, and output brightness
  applies after the capture.
- **`fresh: false`** (the CLI warns; REST sends `X-Canvas-Fresh: 0`) means nothing
  rendered during the wait, so the image is the last frame published. A burst
  stops at the first stale frame.
- **Identical frames across a burst usually mean the script threw** inside its
  animation callback: one uncaught error ends the `requestAnimationFrame` chain.
  The console has the error.
- **Console locations point at your file.** The engine loads a copy of your page
  with an injected preamble; messages are rewritten to the author's path and
  line, and each entry carries `line` and `source_file`.
- **`truncated: true`** means you fell more than 512 messages behind. Do not log
  every frame.

## The page contract

The validator rejects a page missing any of these, and lists every problem at
once: a `<title>` (the catalog name), a `<canvas id="exCanvas">` (that exact id),
and at least one inline `<script>`. Keep everything inline: a rig without network
access cannot fetch an outside script, and the SDK validator flags one.
`references/starter.html` is a complete page that follows every rule below; copy
it rather than starting blank.

- **Size the canvas from the window.** The engine renders at 640x480 by default
  and the user can change it. Set `canvas.width = innerWidth` and
  `canvas.height = innerHeight`, and redo it on `resize`. A fixed
  `width="320" height="200"` fills only the top-left corner of the frame.
- **Declare controls as meta tags.**
  `<meta property="speed" label="Speed" type="number" min="0" max="10" default="5">`.
  Types are `number`, `boolean`, `color`, `combobox` (with `values="A,B,C"`),
  `hue`, and `text`. `min` must be below `max`.
- **Read controls every frame.** The runtime defines each control as a global
  before your script runs and reassigns it on every patch, so
  `const speed = window.speed` at load time freezes the value. Read
  `window.speed ?? 5` inside the frame, or define `window.onspeedChanged`, which
  the runtime calls after each change. Errors thrown inside an `on<name>Changed`
  callback are swallowed without reaching the console, so keep them trivial.
- **Animate on elapsed time.** The render loop shifts between 10 and 60 fps
  under load; use the `requestAnimationFrame` timestamp and clamp the step.
- **Audio needs an explicit opt-in:** `<meta audio-reactive="true">`, then
  `window.engine.audio` (`levelLinear`, `bass`, `beat`, `beatPulse`,
  `frequency`). The daemon does not infer it from your script.

## Designing for LEDs

A gorgeous screen effect often looks like mud on a strip. Check captures against
these before calling it done, and use `format: "png"` when exact colors matter.

- Saturation at 85 to 100 percent, or zero for intentional white. Pastels wash out.
- In vivid areas keep `min(r,g,b) / max(r,g,b)` under 0.3: at least one channel
  near zero.
- Pure yellow reads as white; shift to amber or gold by pulling green below red.
- Features should span 10 or more LEDs. Thin lines, text, fine noise, and bloom
  vanish after sampling.
- Leave a dark floor: 30 to 50 percent of the frame dim or off reads better than
  everything lit.
- Do not gamma correct. The engine owns the transfer curve, and a second one
  crushes the midtones.

## Display faces

LCD faces are effects in the `display` category, rendered into
`<div id="faceContainer">` rather than a canvas. `set_effect` refuses them;
assign one with `set_display_face {device, effect_id}`. `capture_canvas` shows the
LED scene canvas, not a face; read a face's pixels from
`GET /api/v1/displays/{id}/frame`.
