# 22 — Community Vibe Effect Roadmap

**Status:** Reference / Ongoing
**Scope:** Effect curation and design guidelines for SDK effects

## Goal

Create evolved effects that feel like best-in-class community HTML effects: expressive, playful, instantly recognizable, rich controls, and strong defaults that look good without audio.

## Improve, Don't Copy

- We borrow **motifs and control language**, not code or one-to-one compositions.
- Every imported vibe must include at least two improvements:
  - clearer control semantics
  - richer scene/mode switching
  - stronger defaults at idle (no audio required)
  - better palette quality and color separation
  - improved performance at high-density settings
- Visual target: users should recognize the inspiration but still say \"this is better and feels like Hypercolor.\"

## Core Design Rules

- Default to always-on visual quality; audio is optional enhancement, never required.
- Add `Scene`/`Mode` controls for big personality shifts.
- Keep controls simple but meaningful: 6-9 controls total, no dead sliders.
- Use curated palettes and include at least one signature palette per effect.
- Prefer stylized motifs (ribbons, stars, particles, geometric motifs) over generic cloud/plasma noise.

## Extracted Community Patterns

- Poison: dual-direction particle bloom, soft trails, 3-color blend, moody background.
- Falling Stars: gradient sky + simple star blocks, path toggle (vertical/diagonal), size controls density.
- Fireflies: particle life/fade behavior, color mode switch (single/random/rainbow), count and speed.
- Swirl: rotating spiral particle system, rotation modes, color mode, growth control.
- 90's Effect: multi-scene effect with very different motif families under one effect.
- Dark Matter: geometric shards + diagonal void lines + optional tap/ripple accents.
- Electric Space: layered background sweeps + stars + sparks with toggles.
- Nyan Cat: iconic character with position/scale/speed/color-cycle controls.
- Pink Lemonade: split-color world, ripple accents, particle overlay, tap interactions.
- Hyperspace: direction mode, background mode, shape mode, color mode, star count/size/speed.

## Proposed Hypercolor Effects (Must-Have Wave)

### 1) Aurora Hyperspace

- Inspired by: Hyperspace, Electric Space, Dark Matter.
- Visual: center-origin warp streaks, optional bolt sparks, selectable background modes.
- Controls: `Scene`, `Direction`, `Background Mode`, `Star Shape`, `Speed`, `Density`, `Glow`, `Palette`.
- Notes: keep non-audio by default.

### 2) Falling Stars Plus

- Inspired by: Falling Stars, Pink Lemonade.
- Visual: bright star blocks/trails over gradient sky; optional side-color split mode.
- Controls: `Sky Top`, `Sky Bottom`, `Star Color`, `Path`, `Speed`, `Star Size`, `Density`, `Trail`.
- Notes: include a "Pastel" scene and a "Night" scene.

### 3) Fireflies Garden

- Inspired by: Fireflies, Poison.
- Visual: depth-layered firefly swarms with soft glow and breathing alpha.
- Controls: `Color Mode`, `Base Hue`, `Count`, `Size`, `Speed`, `Wander`, `Glow`, `Background`.
- Notes: add "Calm", "Swarm", "Pulse" scenes.

### 4) Dark Matter X

- Inspired by: Dark Matter, Electric Space.
- Visual: geometric shards, shadow bands, neon line sweeps, optional spark bolts.
- Controls: `Shard Density`, `Line Sweep`, `Void Strength`, `Spark Toggle`, `Speed`, `Palette`, `Glow`.
- Notes: include tap/ripple for interactive hosts.

### 5) Lava Lamp Superfluid

- Inspired by: Lava Lamp.
- Visual: metaball blobs with buoyancy, merging/splitting, warm interior gradients, glassy bloom.
- Controls: `Blob Count`, `Blob Size`, `Buoyancy`, `Viscosity`, `Flow Speed`, `Glow`, `Palette`, `Mode`.
- Improvements over classic:
  - true merge/split behavior instead of simple noise blobs
  - mode switch (`Classic`, `Neon`, `Psychedelic`) with tuned defaults

### 6) Bubble Garden

- Inspired by: Bubbles, Fireflies.
- Visual: layered bubbles with refraction tint, soft highlights, variable rise currents, occasional pops.
- Controls: `Bubble Count`, `Size Range`, `Rise Speed`, `Drift`, `Refraction`, `Pop Rate`, `Background`, `Palette`.
- Improvements over classic:
  - depth layering with near/far bubble behavior
  - multiple movement scenes (`Calm`, `Fizz`, `Storm`)

## Proposed Hypercolor Effects (Character Wave)

### 7) Swirl Reactor

- Inspired by: Swirl.
- Visual: multi-arm orbital particle spirals with rotation modes.
- Controls: `Arms`, `Spawn`, `Particle Size`, `Growth`, `Rotation Mode`, `Color Mode`, `Cycle Speed`, `Background`.

### 8) Retro Roller Rink Carpet

- Inspired by: 90's Effect.
- Visual: roller-rink-carpet motifs: looping squiggles, speckled geometry, neon confetti fields, and drifting 90s shapes.
- Controls: `Scene`, `Front Color`, `Accent Color`, `Background`, `Color Mode`, `Cycle Speed`, `Move Speed`.

### 9) Nyan Dash

- Inspired by: Nyan Cat.
- Visual: stylized sprite + rainbow trail + star pops.
- Controls: `Animation Speed`, `Scale`, `Position X/Y`, `Trail Mode`, `Color Cycle`, `Cycle Speed`.

## Proposed Hypercolor Effects (Flavor Wave)

### 10) Pink Lemonade Split

- Inspired by: Pink Lemonade.
- Visual: split-color composition with ripples and floating particles.
- Controls: `Left Color`, `Right Color`, `Ripple Intensity`, `Particle Amount`, `Speed`, `Blend`.

### 11) Poison Bloom

- Inspired by: Poison.
- Visual: dual-flow expanding particle circles, smoky low-alpha accumulation.
- Controls: `Background`, `Color 1/2/3`, `Speed`, `Bloom`, `Spread`, `Density`.

## Control Standard (for all new vibe effects)

- `Scene` or `Mode` when effect has more than one recognizable look.
- `Palette` always present with at least 5 curated options.
- `Speed` and `Glow` always present.
- Density/count controls should cap intelligently to avoid empty/saturated extremes.

## Recommendation

Build in this order:

1. Aurora Hyperspace
2. Falling Stars Plus
3. Fireflies Garden
4. Lava Lamp Superfluid
5. Bubble Garden
6. Dark Matter X

This gives immediate coverage of the strongest requested vibes while establishing reusable primitives (streak field, swarm particles, scene-switch architecture).
