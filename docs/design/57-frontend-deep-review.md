# 57 — Frontend Deep Review: Multi-Zone, Scenes, Performance, Aesthetics, Architecture

**Date:** 2026-06-09
**Scope:** `crates/hypercolor-ui/` (~52k lines), cross-checked against `hypercolor-types` and the daemon API.
**Method:** Five parallel review passes — multi-zone scene support, scene selection, performance, Luminary aesthetics/interaction, architecture judo.

This document is the full findings record. The executive summary and sequencing live at the end (§6).

---

## 1. Multi-Zone Scene Support

Scenario reviewed: a two-zone scene, each zone running its own effect.

### 1.1 Platform reality (what already works)

- `Zone` (`hypercolor-types/src/scene.rs:81-134`) carries per-zone `effect_id`, `controls`, `control_bindings`, `preset_id`, `layers`, `layout`, `brightness`, `enabled`, `color`, `role`, plus per-zone `controls_version` / `layers_version`. `Zone::legacy_layer_id()` (scene.rs:421) makes every zone's current effect addressable through the layers API.
- `api/scenes.rs:9-30` — `ActiveSceneResponse` includes full `groups: Vec<Zone>` + `groups_revision`. The UI can see everything about every zone in one fetch.
- `api/zones.rs` — complete zone CRUD, all `If-Match: groups_revision` guarded with typed `ZoneOutcome::Stale` rebase.
- `api/layers.rs:145` — `patch_layer_controls` is the zone-scoped effect-controls primitive, guarded by per-zone `layers_version`.
- `api/effects.rs:175-177` — `ApplyEffectBody.render_group` targets a zone on apply; `ActiveEffectResponse.render_group_id` (line 49) names the zone.
- Daemon WS already has a per-zone preview channel (`zone_preview`, daemon `api/ws/protocol.rs:59,75`; binary frames carry `scene_id` + `zone_id`). The UI never subscribes.
- Capabilities advertised: `multi-zone-sampling`, `zone-crud`, `zone-device-assignment` (daemon system.rs:29-31).

**Daemon gaps** (the only two real ones — daemon is ~85% zone-capable):
1. `EffectStarted`/`EffectStopped` events carry no zone id (`hypercolor-types/src/event.rs:503,514`); the apply success path even publishes `previous` = the *primary* zone's effect (daemon `api/effects.rs:878-885`). `RenderGroupChanged` (event.rs:590) IS zone-tagged — use it instead until fixed.
2. Zone-scoped preset apply / controls reset / "active effect" queries don't exist: `GET /effects/active`, `PATCH /effects/current/controls`, `/effects/current/reset`, `POST /library/presets/{id}/apply`, `POST /effects/stop` are all primary-group-only (daemon effects.rs:279-297, 924, 1006, 1274; library/presets.rs:193-223). `PATCH /effects/{id}/controls` resolves the *first group whose effect_id matches* (effects.rs:1250-1265) — ambiguous with duplicate effects across zones.

### 1.2 The reference implementation: `/studio`

`pages/studio/` is the pattern library to promote globally:
- `StudioContext` (studio/mod.rs:77-107): `selected_surface_id` (focused zone), `active_scene`, `refresh_scene` callback.
- `Surface` model (studio/surface.rs:27-49): pure, Leptos-free projection of `Zone`; `surfaces_from_groups()` (line 75) is the single zone→UI mapping.
- Per-zone now-playing: `NowPlayingChip` (stage.rs:419-451).
- Zone-scoped composition: shared `LayerPanel` pinned to `(scene_id, group_id)`; `EffectControlsSection` (layer_panel/controls.rs:30) edits via `patch_layer_controls`.
- Apply-target bridge (mod.rs:154-180) writes the selected zone into global `EffectsContext.apply_target`.

**Gap even in Studio:** it never consumes `ws.last_scene_event` — only its own mutations refresh. External zone changes (CLI, other tab, Effects page) leave it stale.

### 1.3 Surface-by-surface verdicts

| Surface | Files | Current behavior | Severity |
|---|---|---|---|
| EffectsContext global state | `app.rs:140-187`, `app/effect_state.rs` | All active-effect state singular = primary zone only; prefs restore writes primary | **BROKEN** |
| WS active-effect stream | `ws/messages.rs:556-563,717-758`, `ws/connection.rs:77` | Name-only signal; `extract_scene_event_hint` **drops `group_id`/`scene_id`** the daemon already sends; stop clears globally; `kind` field collision (735-739 vs 753-756) | **BROKEN** |
| Dashboard favorites/hero | `dashboard/charts.rs:235-526`, `mod.rs:490` | Apply with no zone concept (silently honors invisible `apply_target`); now-playing equalizer = primary only, zone-2 favorite shows as not playing | **BROKEN** |
| Effects page controls panel | `pages/effects.rs:171-222,887-962` | After applying to zone 2 via selector, panel still shows/edits the PRIMARY zone's effect — most misleading interaction in the app | **BROKEN** |
| Preset panel | `preset_panel.rs:227-277` | Preset apply/reset primary-only; no zone-2 preset path exists at all | **BROKEN** |
| Sidebar now-playing/player | `sidebar.rs:38,101-145,331-495` | One effect shown of two; pause stops primary only while zone 2 keeps rendering; shuffle hits hidden zone target | **BROKEN** |
| Command palette | `shell.rs:99-269` | One-click apply, zero zone visibility | **BROKEN** |
| Effects page apply + grid | `effects.rs:59-117,377,787` | `ApplyTargetSelect` zone picker exists (good); grid active-state primary-only; `effects_scene` fetched once → stale | DEGRADED |
| Preview cabinet | `preview_cabinet.rs:75-112,427` | Composed canvas correct; overlay metadata/preset strip primary-only; ignition fires only on primary changes | DEGRADED |
| Effect card | `effect_card.rs:68-77` | Props-driven; no zone badge affordance | DEGRADED |
| Thumbnail auto-capture | `thumbnails.rs:253-330` | Composed multi-zone canvas attributed to the primary effect — cross-contamination | DEGRADED |
| ApplyTarget mechanism | `apply_target.rs`, `app.rs:186` | Works, but invisible cross-page mutable state; cleanup only when Studio mounted | DEGRADED |
| PreferencesStore | `preferences.rs` | Keyed by effect id only; same effect in 2 zones clobbers; restore hits primary | DEGRADED |
| Studio | `pages/studio/*` | Zone-aware reference; no WS-driven refresh; zone_preview unused | OK (minor gaps) |
| Layer panel | `layer_panel/*` | Fully zone-scoped — the contract to copy | OK |
| Control panel renderer | `control_panel/*` | Pure renderer, zone-correct when fed correctly | OK |
| Assets / Media / Settings / Devices / Displays pages | various | No singular assumptions (displays already zone-aware) | OK |
| Dashboard metrics, status strip | `dashboard/{gauges,timeline,renderer,header}.rs` | Engine telemetry, zone-agnostic; `MetricsTimeline.render_group_count` already plumbed (ws/messages.rs:226) | OK |

Other notable findings:
- `apply_effect` AllZones path (`effect_state.rs:26-67`) never applies saved preferences/presets, unlike the primary path (`app.rs:264-273`) — inconsistent between targets.
- `scene_event_affects_active_effect` (ws/messages.rs:760-762) treats every non-display `render_group_changed` as affecting "the" active effect → over-fetch, can't drive per-zone updates.
- Per-zone layer health (ws/messages.rs:669-715) is already correctly zone-keyed.

### 1.4 Recommended architecture

**(a) Global `ZonesContext`** — promote Studio's scene state to `app.rs`:

```text
ZonesContext {
    active_scene: Signal<Option<ActiveSceneResponse>>,
    zones: Memo<Vec<Surface>>,          // surfaces_from_groups, moved out of pages/studio
    led_zones: Memo<Vec<Surface>>,
    multi_zone: Memo<bool>,
    focused_zone: RwSignal<Option<String>>,   // replaces invisible apply_target signal
    refresh: Callback<()>,
}
```

Wire `ws.last_scene_event → refresh` (debounced) after extending `SceneEventHint` with `group_id`/`scene_id` (UI-only fix in `ws/messages.rs:717` — daemon already sends them). Fixes Studio staleness for free.

**(b) Per-zone `EffectsContext`** — replace the singular `active_*` quintet:

```text
ZoneEffectState { effect_id, effect_name, category, controls, control_values,
                  preset_id, controls_version, layers_version, enabled }
zone_effects: Memo<HashMap<ZoneId, ZoneEffectState>>   // derived from active_scene
focused_zone_effect: Signal<Option<ZoneEffectState>>
```

Most state is derivable from the scene (Zone serializes effect_id/controls/preset_id/versions, scene.rs:136-160); only control *schemas* need per-effect `fetch_effect_detail` (cacheable). Keep thin compat signals (`active_effect_id` = primary) during migration.

**(c) Zone-routed writes:**
- Apply: keep `render_group`, make target explicit at every call site (chip/picker), default = `focused_zone`. Fix AllZones preference baking.
- Controls: route focused-zone edits through `patch_layer_controls(scene, zone, zone.legacy_layer_id(), …)`; key `OptimisticControlSession` per `(zone, layer)`.
- Presets: daemon `render_group` on preset apply + reset — or client-side translation of preset controls into a layer-controls patch (viable today).
- Per-zone pause: `update_zone { enabled: false }` (api/zones.rs:130), not `/effects/stop`; reserve global stop for "stop all".

**(d) Per-surface UX:**
- Dashboard: per-zone now-playing chips (zone swatch + name + effect); favorites get hover "Apply ▸ zone" + visible target chip; `is_active` = "running in N zones".
- Effects page: controls panel gets a zone tab strip; per-zone active rings on cards.
- Sidebar: per-zone rows under Now Playing (cap 3, overflow → Studio); player controls act on visible focused zone.
- Palette: target chip + "apply to…" submenu + zone-focus commands.
- Thumbnails: gate auto-capture on single-zone scenes, or capture from `zone_preview`.
- Preferences: key by `(effect_id, zone)` or defer to scene-persisted zone controls.

**(e) Migration order:**
1. UI-only: `SceneEventHint.group_id`, global `ZonesContext`, WS-driven scene refresh.
2. UI-only: per-zone `EffectsContext` derivation + sidebar/dashboard per-zone now-playing (read-only — immediately stops the UI lying).
3. UI-only: zone-scoped controls via layer path; effects-page zone tabs; visible target chips.
4. Daemon: zone ids on effect lifecycle events; zone-scoped preset apply/reset.
5. Polish: `zone_preview` subscriptions; per-zone pause via `enabled`.

---

## 2. Scene Selection (>1 saved scene)

### 2.1 Current state

With multiple scenes defined, the active scene is *visible* in exactly two places (Dashboard pill — stale; Effects warning banner — only when a named scene is active) and *switchable* in exactly one (Studio toolbar).

| Surface | Shows | Switch? | Freshness |
|---|---|---|---|
| Studio `scene_selector.rs` | Full picker + New/Rename/Delete | **Yes** (only one) | Own mutations only; no WS — stale on external activation |
| Dashboard `header.rs:60-76` | Scene name pill, lock variant | No | Stale: `SystemStatus` fetched once (`dashboard/mod.rs:179`) |
| Sidebar | Nothing | No | — |
| Command palette `shell.rs:99-141` | Nothing (effects-only search) | No — gap | — |
| Effects page | Named-scene warning banner (286-292, 700-734) + "Return to Default" (`deactivate_scene`, 403-409); `ApplyTargetSelect` zones | Only escape-to-Default | Banner fresh (WS via `app.rs:737-770`); zone options stale (`effects_scene` fetch-once, line 377) |
| Displays / display_preview | No identity; WS-reactive face refresh | No | Fresh |
| Devices `devices.rs:110-117` | Zone-membership badges | No | Stale (fetch-once) |
| Assets `assets.rs:30-48` | Zone selector + layers | No | Stale on external switch |

### 2.2 API/WS gaps

- UI `SceneSummary` (api/scenes.rs:40-46) drops daemon fields `enabled`, `priority`, `mutation_mode` (daemon scenes.rs:56-64) — `mutation_mode` would let a switcher mark snapshot-locked scenes without joining `/scenes/active`.
- UI rename sends only name/description; daemon `PUT /scenes/{id}` also accepts `enabled` + `mutation_mode` — no UI path to toggle snapshot lock.
- `active_scene_changed` WS event carries id/name/kind/mutation_mode/snapshot_locked (daemon api/mod.rs:873-890) — enough to update a switcher with zero refetch — but no `groups`, so group-dependent surfaces must refetch.
- **No WS event for scene create/rename/delete-of-inactive** (daemon publishes only on activate/deactivate/delete-of-active). True list-sync needs a new daemon event; a global context can refetch the list on `active_scene_changed` as a stopgap.
- No daemon duplicate-scene endpoint exists.

### 2.3 Recommended architecture

Global `ScenesContext` in `app.rs`:

```text
ScenesContext {
    scenes: Signal<Vec<SceneSummary>>,            // list, WS-refreshed
    active: Signal<Option<ActiveSceneResponse>>,  // single shared resource (merge with ZonesContext.active_scene)
    switching: ReadSignal<Option<String>>,        // scene id mid-activation
    refresh: Callback<()>,
    activate: Callback<String>,
    deactivate: Callback<()>,
}
```

Consumers: Studio selector (keeps CRUD UI, drops private resource — fixes its staleness); Dashboard pill → interactive `SilkSelect`-backed switcher (keep lock styling); command palette scene entries (`scene ` prefix or `@`); a compact sidebar footer chip with popover, rendered only when `scenes.len() > 1`; Effects `ApplyTargetSelect` and Devices/Assets point at the shared resource.

Note: the shell has no global header (`shell.rs:73-96`) — the app-wide affordance slot is the Sidebar.

### 2.4 Switching UX

- **No optimistic commit** — activation rewrites zones wholesale and can fail validation (daemon scenes.rs:316-321). Pattern: `switching = Some(id)` → spinner on target row → flip on WS hint/refetch → toast on error.
- Canvas crossfade is already free (`scene_transition_active`, ws/messages.rs:228) — don't add a UI-side preview fade.
- Snapshot-locked scenes: activatable but marked with lock glyph.
- The ephemeral default is never in the list (daemon filters `SceneKind::Ephemeral`); offer an explicit "Default" row mapped to `deactivate_scene` (mirrors the banner's "Return to Default").
- Render nothing new when ≤1 scenes.

---

## 3. Performance (no nerfs — waste removal only)

### Top 5 wins

1. **H1 — Thumbnail auto-capture does heavy work per frame, forever** (`thumbnails.rs:268-338`, `app.rs:602-614`). The Effect subscribes to `canvas_frame` (up to 60 Hz). Steady state per frame: linear effects-index scan + two String clones + `ThumbnailStore::get` that clones the entire Thumbnail **including the 15-80 KB base64 WebP data_url** just to check existence (`thumbnails.rs:96-99`). ~2.5 MB/s allocation churn. Fix: cache a settled marker in `StoredValue`; existence check via `with_untracked(|m| …is_some_and(…))`; or throttle to ~1 Hz like the frame-analysis pass (`app.rs:513-527`).
2. **M1 — Dashboard clones full `PerformanceMetrics` (~250 fields) per memo per tick** — ~80 memos across `gauges.rs`, `charts.rs`, `timeline.rs`, `renderer.rs` each call `metrics.get()` → ~160 full-struct clones/sec. Fix: `.with()`/`.read()` projections, or one `Memo<SmallProjection>` per panel.
3. **M3 — `SidebarAudioToggle` rebuilds its DOM at ~10 Hz** (`sidebar.rs:644-681`) on every page when audio is on — recreates button + Icon subtree per audio tick. Fix: static DOM + `Memo<(color, shadow)>` (values are quantized to 3 tiers, so the memo dedupes) bound via `style:`. Counter-example done right: `settings_sections/audio.rs:48-119`.
4. **M4 — Thumbnail capture crosses the WASM/JS boundary ~1.2M times synchronously** (`thumbnails.rs:180-191` per-pixel `rgba_at`). Fix: one `copy_to` memcpy + RGB→RGBA expand in WASM — pattern already exists in `preview_runtime/canvas2d.rs:8-52`.
5. **M5 — `PhaseWaterfall` rebuilds ~420 absolutely-positioned divs every 500 ms** (`perf_charts.rs:392-547`; also `StackedBar` 293-339, `DistributionBar` 565-641). Fix: fixed column slots with reactive per-slot styles, or SVG paths/canvas mutation. Same visuals, same 2 Hz.

### Medium/Low

- **M2** — `renderer.rs:95-487`: ~50 `Signal::derive` props, no equality gating → every tick re-runs all closures and re-touches DOM. Convert to Memos (pairs with M1).
- **M6** — WS handlers deep-clone the full JSON tree before deserializing (`ws/messages.rs:507,519,536` — `from_value(msg.clone())`). Fix: `MetricsMessage::deserialize(msg)` (serde_json implements `Deserializer` for `&Value`).
- **L1** — `resolved_aspect_ratio` memo recomputes per frame (`canvas_preview.rs:505-514`); stage via `Memo<(u32,u32)>`.
- **L2** — per-card thumbnail `Signal::derive` chains re-clone the WebP data URL on every store insert (`effect_card.rs:96-122`, `charts.rs:373-416`); store palettes separately from image data.
- **L3** — `capabilities` rebuilds a HashSet + clones status per read (`app.rs:504-510`); make it a Memo.
- **L4** — `ThumbnailStore::persist` rewrites the whole localStorage blob per insert, unbounded growth (`thumbnails.rs:103-131`); add LRU cap.

### Verified good (leave alone)

Binary frame decode is zero-copy subarray views; WebGL runtime caches context/program/texture and uses `texSubImage2D`; RAF-latched latest-value presentation with frame-number dedupe; worker runtime coalesces and reuses scratch buffers; demand-driven preview subscriptions with dedupe + hidden-tab gating + 64px-quantized width caps; shared 2 Hz frame analysis feeding shell hue + sidebar palette with change-gated CSS-var writes; event-driven refetch everywhere except a panel-scoped 2 s sensor poll; control panel memoizes structure not values; control PATCHes debounced 75 ms with epoch guards; metrics signal equality-gated; keyed `<For>` on the effects grid; build already `opt-level=z` + LTO + wasm-opt z.

---

## 4. Aesthetics & Interaction (Luminary audit)

### Cross-cutting, by visual impact

1. **Phantom classes that silently render nothing** (verified against built CSS): `bg-surface-panel` (layer_panel/picker.rs:68 — the Add Layer modal has NO background); `bg-border-subtle/30` ×6 (sidebar.rs:293, effects.rs:566,627,680, devices.rs:407,441 — all six dividers invisible; correct alias is `bg-edge-subtle/*`); `hover:bg-accent-bright` (app.rs:829); `animation: fade-in` ×8 (settings.rs:337-398 — the keyframe is `enter-fade`; Settings' entire entrance cascade never runs).
2. **Ambient reactivity is dead wiring.** `shell.rs:25-42` computes `--ambient-hue` every tick; the only CSS consumer is `.preview-glow` (input.css:861-866) which **zero components reference**. The preview cabinet uses neutral `edge-glow` (preview_cabinet.rs:165-168). The signature §9 behavior renders nowhere.
3. **Light theme broken at the token layer:** `--glow-focus`/`--glow-focus-soft` only defined in dark block (semantic.css:66-67) → focus rings effectively invisible on white; `::selection` white-on-pale (input.css:186-189); Leptoaster theme `:root`-only raw hex (input.css:7-30); `--status-warning/success` never darkened; hardcoded dark surfaces in viewport_picker.rs:391, preset_panel.rs:546-548/619-624, settings.rs:276 (near-white active-tab text), dashboard panel_frame.rs:100-128 (`bg-black/55 border-white/8`), display_preview.rs:120,164.
4. **Raw-color sprawl: 552 raw `rgba(` lines across 68 files.** Worst literals: warning/error banners hand-painted with arbitrary values, duplicated wholesale (effects.rs:702-721 ≡ displays.rs:324-345); identical filter-button purple rgba copy-pasted (effects.rs:516-530 ≡ devices.rs:369-377); page_header.rs:48-53 hex gradients; welcome_overlay.rs all-inline; input.css itself uses raw purple in `.modal-glow`/`.dropdown-glow`/`.resize-handle-*`/`.scrollbar-dropdown`; legacy aliases (`bg-electric-purple` ×7, `text-neon-cyan` sidebar player, `var(--color-electric-purple)` dashboard pills) despite the guide's "do not use".
5. **Accent discipline (§4: purple is the only chrome accent):** Settings tabs cyan (settings.rs:285-301); Devices Scan cyan (devices.rs:344-347); Media Upload coral (media.rs:157-161); filter checkboxes coral/cyan (effects.rs:575-617); control panel rotates a rainbow per group header (`SECTION_COLORS`, control_panel/mod.rs:59-66).
6. **Radius violations (§14.2 max 8px):** `rounded-2xl` (picker.rs:68, viewport_picker.rs:360); `rounded-[1.5rem]`…`[0.95rem]` (viewport_picker.rs:391-422); `rounded-[2rem]` (display_preview.rs:120,164). Otherwise disciplined (183× rounded-lg, 137× rounded-md, 93× rounded-xl).
7. **Typography drift:** `text-[8px]` ×17, `text-[13px]` ×11, off-token 21px/500 'Sora' page title via inline style (page_header.rs:86-90; Sora isn't in the token system); ~150 `text-[10px]`/137 `text-[11px]` ad-hoc labels bypassing `section_label.rs`; conflicting `uppercase tracking-wider capitalize` (effect_card.rs:305). Guide is stale both directions (documents unshipped logo modes/fonts; doesn't document Sora or 9px Micro).
8. **Diverged duplicates:** `EmptyState` (assets.rs:484-497) ≡ `MediaGridEmpty` (media_grid.rs:167-176) byte-identical; scene banners duplicated (effects/displays); filter dropdown duplicated (effects/devices); docked vs detached Controls header diverged within effects.rs (872 vs 942); `FavoriteCinemaCard` parallel-implements `EffectCard` (dashboard/charts.rs:231,327); **five** hand-rolled toggle switches vs the design system's `.toggle-track` (only control_panel/boolean.rs:42 uses it).
9. **Page-accent collisions:** Assets=Purple duplicates Effects; Media=Coral duplicates Studio/Layout (vs page_header.rs:11-12's "distinct identity" promise).

### Interaction-quality gaps

- **Modals:** only the command palette has `role="dialog"` + Escape (shell.rs:156-211). `DevicePairingModal`, `SimulatorModal`, layer `Picker`, `WelcomeOverlay`, `ApiKeyPrompt`: no Escape, no focus trap, no restore, no dialog role. `ApiKeyPrompt` doesn't autofocus (app.rs:810-826). Palette doesn't scroll keyboard selection into view; `on:mouseenter` fights arrow keys (shell.rs:247).
- **Keyboard:** `/` kbd hint wired to nothing (page_search_bar.rs:6,41-44); global shortcuts stop at Ctrl+K/1/2 (shell.rs:46-71); `SilkSelect` has no arrows/Enter/typeahead/listbox role (silk_select.rs:105-188) — used on every page; `ColorWheel` pointer-only; dashboard panel reorder mouse-DnD only.
- **Cursor:** 228 buttons, only 36 `cursor-pointer`; Tailwind v4 preflight doesn't set it. One global `button:not(:disabled){cursor:pointer}` fixes all.
- **Silent failures:** default `apply_effect` reverts without a toast (app.rs:334-340) while the zone path toasts (283); favorite toggle reverts silently (app.rs:353-374); `stop_effect` silent (383-387); `let _ =` swallows in effect_state.rs:210,218, device_detail.rs:113, library_provider.rs:138, devices.rs:350. Devices "Scanning..." toast fires *after* the await (devices.rs:344-352); Media Upload has no busy/disabled state (media.rs:157-165).
- **Hover/contrast:** `filter_chips` — the primary filter control — has no hover state at all (style_utils.rs:110-125); inactive chips at `rgba(rgb, 0.5)` will fail contrast in light mode.
- **Reduced motion (§8.5):** block at input.css:1109-1143 omits `.animate-eq-bar`, `markChillAura` (24 s infinite blur+hue-rotate — also expensive), `.search-glow`, `.animate-swap-in`, `.animate-picker-in`, `fullscreen-preview-in`.
- **A11y:** 35 aria-labels for 228 buttons; favorite heart unlabeled on every card (effect_card.rs:237-265); sidebar brightness slider unlabeled (sidebar.rs:473-488); expanded sidebar logo has pointer affordance but no click handler (sidebar.rs:226-237).
- **404 page:** bare `<p>"Not found"` (app.rs:856) — least-designed surface in the app.

### Top 10 polish moves

1. Fix the four phantom-class families (one-line fixes; restores a modal background, six dividers, a hover state, Settings' entrance).
2. Wire `.preview-glow` onto the preview cabinet + extend `--ambient-border` to scrollbars — make the signature behavior real.
3. Repair light-theme tokens (`--glow-focus*`, `--selection-bg`, light status colors, light Leptoaster block).
4. Global `button:not(:disabled){cursor:pointer}`; delete scattered `cursor-pointer`s.
5. One `Modal` wrapper: backdrop + `role="dialog"` + Escape + focus trap/restore; adopt in all five modals.
6. Shared `StatusBanner` + `EmptyState` components, tokenized; replace the diverged duplicates.
7. Keyboard for `SilkSelect` (arrows/Enter/Home/End + listbox roles) + palette scroll-into-view.
8. Toast on failed apply/favorite/stop; busy states for Scan/Upload; move Scanning toast before the await.
9. Wire `/` to focus search; Ctrl+3…8 for remaining pages; page navigation in the palette.
10. Settings tabs → page accent + tokenized text; pass `index` into `EffectCard` from the grid so the stagger cascade actually plays (effects.rs:784-800 vs effect_card.rs:75-76).

Also: add missing animations to reduced-motion block; reconcile DESIGN-SYSTEM.md with shipped reality; de-rainbow `SECTION_COLORS`; persist sidebar collapse state.

Foundations verified strong: token tiers, `section_label.rs`, `PageSearchBar`/`PageHeader`/`PanelFrame`, consistent skeletons on every data page, well-crafted micro-interactions (`card-hover`, `btn-press`, ignition). The gap is adoption discipline, not system design.

---

## 5. Architecture Judo

Ranked by leverage:

**Move 1 — Promote the app into the lib target; kill `#[path]` test hacks.** lib.rs exposes 6 modules; main.rs declares 37 others *including second copies* of `apply_target`/`label_utils`/`tauri_bridge` (dual type identities; api/controls.rs:8-11 imports `hypercolor_ui::` while siblings use `crate::`). All 29 test files (4,862 lines) smuggle sources via 50 `#[path]` includes; api/mod.rs recompiles into 13 test binaries. Move: all mods into lib.rs; main.rs ≈ 15 lines. Near-zero risk, unlocks compiler-verified dead-code audit + cheap tests for everything else.

**Move 2 — Generic preconditioned-mutation helper (fixes a latent auth bug).** The If-Match/412-Stale pattern is hand-rolled 3× (`zones.rs:256-296`, `layers.rs:188-225`, `effects.rs:226-269`) with 3 outcome enums. The bypasses skip `client::with_auth`: **effects.rs:236, effects.rs:284, devices.rs:330/391/412, layouts.rs:114 send no Authorization header** — these 401 the moment the daemon's network API-key requirement is on. Move: `MutationOutcome<T>` + `send_json_versioned(method, url, body, if_match)` in client.rs; route raw callsites through helpers. ~120 lines → ~45, 5 auth fixes.

**Move 3 — One mutation-spawning helper.** `async_helpers.rs` exists with 6 callers vs **112 `spawn_local` sites**, ~63 in the same `match { Ok => act, Err => toast }` shape, 56 `toast_error(&format!(…))`. Recurring sub-shapes: toast+refetch, optimistic rollback (app.rs:344-376 favorites, preset_panel.rs:226-277 ×3, app.rs:309-340 via capture/restore), busy-flag wrap. Move: `spawn_mutation(fut, on_ok, on_err)` + `with_rollback(undo)` variant — also the generic answer to the snapshot/rollback duplication. ~250-400 lines.

**Move 4 — Shared API-DTO crate.** `src/api/*.rs` hand-mirrors ~85 daemon DTOs; drift is real (daemon `ZoneSummary.led_count: u32` vs UI `usize`; UI needs private `WireActiveEffectResponse` to re-derive daemon shapes). `hypercolor-types` already depends on utoipa, so `ToSchema` derives can move. Extract to `hypercolor-types::api` or a thin `hypercolor-api-types` crate; migrate domain-by-domain (devices first, 22 types). ~500-700 lines; serde drift becomes a compile error.

**Move 5 — `use_control_patch_session` hook.** The debounced-optimistic-controls session is reimplemented 4×: effects.rs:168-222 (~85 lines, 75 ms), displays/face_controls.rs (318 lines — also duplicates effect_state.rs's prefs restore state machine), layer_panel/controls.rs:64-118 (120 ms, ad-hoc pending store), viewport_designer.rs:145-186. One hook over `(defs, values, patch_fn, debounce_ms)`; `MutationOutcome` from Move 2 makes the three PATCH routes plug-compatible. ~300 lines; every future control surface free. Preserve existing cadences (perf baseline rule). **Prerequisite for zone-tabbed controls in §1.**

**Move 6 — Split `WsContext`; delete the 24-field mirror.** `WsManager` (ws/connection.rs:57-95) and `WsContext` (app.rs:55-96) are the same 24 fields copied one-by-one (app.rs:450-476), bundling ≥4 concerns. Move: sub-structs (`PreviewStreams`, `EventHints`, `WsMetrics`) provided as separate contexts; add `use_preview_stream(fps_cap, width_cap)` (subscribe + on_cleanup dance copy-pasted in 6 files) and `use_ws_event(hint, handler)` (prev-compare idiom, 9+ occurrences).

**Move 7 — `EffectsContext` on `RwSignal`s + `ActiveScene` struct.** 12 read/write pairs = 24 fields (app.rs:141-187) vs the newer `StudioContext` RwSignal pattern at half the size; the scene name/kind/mutation_mode triple is one value masquerading as three signals; capture/restore trio (~70 lines, effect_state.rs:231-302) collapses to struct get/set. **Do this as part of the §1.4 per-zone refactor, not separately.**

**Move 8 — Smaller (opportunistic):** client.rs sextuplet collapse (~65 lines, natural home for Move 2's header hook); assets/media 22-line filter/sort exact copy (or schedule AssetsPage deletion — MediaPage is its successor, flag default-on); shared `matches_query` (4 hand-rolled search sites); `<ResourceView>` for ~12-14 `match resource.get()` triples; burn down `#![allow(dead_code)]` tombstones (api/zones.rs:14, api/controls.rs:2, layout_geometry.rs:2-5, layer_panel/source.rs:252). Layout subsystem: no major move needed (state split is sane; no duplication with `hypercolor-types/src/spatial.rs`).

Aggregate: ~1,200-1,700 lines of boilerplate removable; 5 missing-auth callsites fixed; new endpoint / mutation flow / control surface drop from ~60-100 lines of ceremony to ~5-15.

---

## 6. Executive summary & sequencing

**The headline:** the daemon and the data model are already ~85% multi-zone/multi-scene capable, and Studio + LayerPanel contain working reference implementations of everything needed. The rest of the UI simply never adopted them — global state is keyed to "the one active effect" (= primary zone), and scene awareness lives in exactly one toolbar. Separately: four phantom CSS classes silently break real surfaces today, the light theme is broken at the token layer, the signature ambient-glow system computes every tick but renders nowhere, the thumbnail system burns ~2.5 MB/s on every frame forever, and five raw API callsites are missing auth headers.

**Recommended phases:**

| Phase | Work | Source |
|---|---|---|
| **0 — Bug fixes (days)** | Phantom classes; missing-auth callsites; silent-failure toasts; perf H1 (thumbnail per-frame work); WS `from_value(clone)` | §3 H1/M6, §4.1, §5 Move 2 (auth part), §4 interaction |
| **1 — Foundations** | Move 1 (lib promotion); Move 2 (`MutationOutcome`); Move 3 (`spawn_mutation`); `SceneEventHint.group_id/scene_id` | §5, §1.4(a) |
| **2 — Zone+Scene state** | Global `ZonesContext` + `ScenesContext` (merge `active_scene` resource); per-zone `EffectsContext` (with Move 7); WS-driven refresh; kill page-local stale `fetch_active_scene` resources | §1.4, §2.3 |
| **3 — Zone+Scene surfaces** | Per-zone now-playing (sidebar, dashboard, cabinet); zone-tabbed controls (needs Move 5); visible apply-target chips; scene switcher (dashboard pill, palette commands, sidebar chip); per-zone effect-card badges | §1.4(d), §2.3 |
| **4 — Daemon support** | Zone ids on effect lifecycle events; zone-scoped preset apply/reset; scene-list-changed event; `SceneSummary.mutation_mode` passthrough | §1.1, §2.2 |
| **5 — Polish wave** | Aesthetics top-10 (§4); reduced-motion; Modal wrapper; SilkSelect keyboard; remaining perf M1-M5; `zone_preview` thumbnails | §3, §4 |

Review performed by five parallel agents on 2026-06-09; this doc is the canonical record of their findings.
