# 58 — TUI Deep Review: Multi-Zone, Scenes, Performance, Input Modes, Architecture

**Date:** 2026-06-09
**Scope:** `crates/hypercolor-tui/` (~15.8k lines), cross-checked against `hypercolor-types` and the daemon API.
**Method:** Three parallel review passes (multi-zone/scene gaps, performance, architecture + input modes), findings verified against source before recording. Companion to doc 57 (frontend deep review) — the daemon capability map there (§1.1) applies here unchanged.

---

## 1. Multi-Zone Scene Support

The TUI predates zones entirely. Its world model is one active effect + one scene *name*:
`DaemonState { effect_name, effect_id, scene_name, scene_snapshot_locked, … }` (`state.rs:42-55`).
There is no zone type, no scene list, no groups fetch, no `render_group` on apply — every
surface below inherits that.

### 1.1 Surface-by-surface verdicts

| Surface | Files | Current behavior | With a 2-zone scene | Severity |
|---|---|---|---|---|
| App state | `state.rs:26-55` | Singular `effect_name`/`effect_id`/`scene_name` | Zone 2 invisible everywhere | **BROKEN** |
| REST client | `client/rest.rs` | No scenes/zones/layers endpoints at all | Cannot even fetch zone data | **BROKEN** |
| Apply path | `rest.rs:234-250`, `app.rs:777-808` | `POST /effects/{id}/apply` — no `render_group` | Always silently retargets the primary zone | **BROKEN** |
| Control edits | `rest.rs:271-291`, `effect_browser.rs` controls pane | `PATCH /effects/current/controls`, `POST /effects/current/reset` | Edits the primary zone while user thinks they're editing the selected effect; zone-2 controls unreachable | **BROKEN** |
| Dashboard "Now Playing" | `dashboard.rs:101-377` | Single effect via `daemon_state.effect_id` | Shows one of two running effects | **BROKEN** |
| Status bar | `chrome/status_bar.rs:104-152` | One gradient effect name + scene name | One of two effects shown | **BROKEN** |
| Fullscreen preview info bar | `app.rs:1410-1422` | `daemon.effect_name` | Mislabels the composed multi-zone canvas with the primary effect | DEGRADED |
| WS bridge events | `bridge.rs:161-196` | `effect_*` → full refetch; `active_scene_changed` → name merge only; `render_group_changed` ignored | Zone mutations from web UI/CLI never refresh the TUI | **BROKEN** |
| WS subscription | `client/ws.rs:48-56` | `canvas, spectrum, events, metrics` | No `zone_preview`; composed canvas only (acceptable) | OK (gap noted) |
| Effect browser controls seed | `effect_browser.rs:260-274` | Defaults from effect detail (primary zone's `active_control_values`) | Zone-2 values never shown | **BROKEN** |
| Device manager | `views/device_manager.rs` | Device-centric, zone-agnostic | No zone membership shown | DEGRADED |
| Canvas preview | `preview.rs` | Composed canvas, zone-agnostic | Correct (it's the real composite) | OK |

### 1.2 The daemon API the TUI must adopt (verified against the router, `daemon/api/mod.rs:1172-1240`)

All paths prefixed `/api/v1`; responses wrapped in `{ data, meta }`.

| Need | Call | Notes |
|---|---|---|
| Scene list | `GET /scenes` | `{ items: [SceneSummary { id, name, description, enabled, priority, mutation_mode }], pagination }`; ephemeral default filtered out |
| Active scene + zones | `GET /scenes/active` | `ActiveSceneResponse { id, name, kind, mutation_mode, groups: Vec<Zone>, groups_revision, … }`; 404 when none |
| Activate | `POST /scenes/{id}/activate` | refetch active scene after |
| Return to default | `POST /scenes/deactivate` | mirrors web UI's "Return to Default" |
| Zone-targeted apply | `POST /effects/{id}/apply` body `{ controls?, preset_id?, render_group? }` | `render_group` = zone id; omitted = primary (`effects.rs:58-76`) |
| Zone enable/brightness | `PATCH /scenes/{id}/zones/{zone_id}` body `UpdateZoneRequest { enabled?, brightness?, … }` | `If-Match: <groups_revision>` optional; 412 on stale |
| Zone-scoped controls | `PATCH /scenes/{id}/groups/{group_id}/layers/{layer_id}/controls` body `{ controls }` | `layer_id` = `Zone::legacy_layer_id()` = the zone's UUID re-tagged (`types/scene.rs:421-423`); `If-Match: <layers_version>` optional; 412 on stale |

`Zone` (types/scene.rs:81-134) serializes `id, name, effect_id, controls, preset_id, brightness, enabled, color, role (custom|primary|display), controls_version, layers_version`, plus layers/layout the TUI can ignore. The TUI already depends on `hypercolor-types`, so it can deserialize `Zone` directly — no hand-mirrored DTO.

WS events on the already-subscribed `events` channel:

- `active_scene_changed` — data: `{ previous, current, current_name, current_kind, current_mutation_mode, current_snapshot_locked, reason }` (`daemon/api/mod.rs:873-890`). Today the bridge merges only name+lock (`bridge.rs:322-354`); it must also refetch `/scenes/active` and the scene list.
- `render_group_changed` — data: `{ scene_id, group_id, role, kind }` (`daemon/api/mod.rs:708-722`). **Completely ignored today**; it is the canonical zone-mutation signal (doc 57 §1.1: effect lifecycle events carry no zone id — don't rely on them).

### 1.3 Recommended architecture

Mirror the frontend's ZonesContext at TUI scale:

- `AppState` gains `scenes: Vec<SceneSummary>`, `active_scene: Option<ActiveScene>` (projection: id, name, kind, snapshot_locked, groups_revision, zones), `focused_zone: Option<ZoneId-string>`.
- New actions: `ScenesUpdated`, `ActiveSceneUpdated`, `ActivateScene(id)`, `DeactivateScene`, `ToggleScenePicker`, `CycleZoneFocus`.
- Apply routes `render_group = focused zone` when non-primary; control edits route through the layer-controls PATCH for the focused zone; reset re-applies zone defaults the same way.
- Bridge: bootstrap fetches scenes + active scene; `active_scene_changed` and `render_group_changed` refetch active scene (and list on scene change).
- Surfaces: dashboard Now Playing becomes per-zone rows when `zones.len() > 1`; status bar shows focused-zone effect + zone count; effect browser title shows the apply target.

## 2. Scene Selection

With >1 saved scene the TUI shows the active scene's *name* in the status bar — and that's all. No list, no switcher, no activate/deactivate, no create/rename/delete. The bridge keeps the name fresh via `active_scene_changed` (`bridge.rs:176-185`), so display freshness is fine; capability is absent.

Recommended: a scene picker modal (same interaction class as the theme picker, which already proves the modal pattern: capture keys while open, Esc closes — `app.rs:378-396`): list scenes + an explicit "Default" row mapped to `POST /scenes/deactivate`, lock glyph for snapshot scenes, Enter activates, refresh on `active_scene_changed`. Render a hint only when `scenes.len() > 1` (doc 57 §2.4 parity).

## 3. Performance (no nerfs — waste removal only)

### H1 — Effect list N+1, re-run on every apply ★ the dominant cost

`get_effects()` (`rest.rs:79-113`) fetches `/effects` then `/effects/{id}` for **every** effect (`buffer_unordered(8)`). That full N+1 runs:

1. on bootstrap (`bridge.rs:154`) — acceptable, it's the schema load;
2. after **every** apply/preset via `refresh_effects_and_status` (`app.rs:783, 801, 1483-1490`);
3. on **every** `effect_*` WS event — including `effect_started`/`effect_stopped` emitted by the apply itself (`bridge.rs:172-175`) — so a single apply triggers the N+1 **twice**.

With a 100-effect library: ~204 HTTP requests + two full list re-sorts per apply, plus a `Vec<EffectSummary>` clone into every screen (`app.rs:654-656` broadcast). Fix: applies should refresh status + active scene only (zone controls live in the scene now); reserve the full N+1 for rescan/install events and bootstrap. No cadence changes.

### Medium

- **M1 — Full-struct clones per broadcast.** Every `EffectsUpdated`/`DevicesUpdated` is `clone_from`-ed into 3+ screens and AppState (`app.rs:654-660`, `dashboard.rs:723-737`, `effect_browser.rs:1580-1587`). The effect-browser additionally clones the filtered list (`apply_filter`, `effect_browser.rs:154-172`) on every keystroke of a search. Filtered *indices* over the shared `Arc<Vec<_>>` would eliminate both. Worth doing opportunistically, not a hot loop at idle.
- **M2 — `DaemonState` cloned per update** (`app.rs:593, 649` + per-screen copies). ~2 Hz × N screens; small but free to fix by sharing the `Arc` (actions already box it).
- **M3 — `get_status` is two sequential round-trips** (`rest.rs:53-76`: `/status` then `/effects/active`). Run concurrently or fold into the active-scene fetch once zones land.

### Low

- Double `drain_resize_results()` per loop iteration (`app.rs:253, 296`) — second call is the one that matters before draw.
- `build_preview_image` copies the frame once via `pixels.to_vec()` (`preview.rs:1142/1155/1166` — mutually exclusive paths, **one** copy per frame). Required by `RgbImage::from_raw`; not worth contorting. *Recorded here because a prior pass misread it as a triple copy.*
- Effects re-sorted on every fetch (`rest.rs:104-110`) — becomes irrelevant once H1 lands.

### Verified good (leave alone)

Binary WS decode is zero-copy (`Bytes::slice`, `ws.rs:185-190`); canvas/spectrum actions are latched to latest-value in the drain loop (`app.rs:256-291`); preview pipeline has adaptive backpressure, protocol fallback, and draw-duration feedback (`preview.rs`); render gating via `render_dirty` + `motion.is_active()` is correct; bridge reconnect is single-notification with 2 s backoff + cancellation; simulator polls are interval-gated (2 s list / 250 ms frame, browser-screen-only); motion reactive channels are lock-free atomics.

## 4. Input Modes & Interaction

### 4.1 Global hotkeys eat text input (the bug class)

`App::handle_key_event` (`app.rs:413-436`) runs global bindings **before** delegating to the screen. While the effect browser's search is active (or its color picker is open), the screen never sees:

`q` (quits the app mid-word!), `t` (theme picker), `m` (motion), `z` (fullscreen), `d e v p s b` (screen switches), `?` (help), `$` (donate).

Typing "purple" into search: `p` → Profiles screen, query lost. Typing "quit" → the TUI exits. There is no input-mode concept; `search_active` exists only inside the view (`effect_browser.rs:71, 1452`).

**Fix (minimal):** add `Component::captures_input(&self) -> bool` (default `false`; effect browser returns `search_active || color_picker.is_some()`). When the active screen captures input, App skips the global block and delegates everything. ~15 lines, fixes the class for every future text surface (scene rename, API key entry…).

### 4.2 Verified correct

Modal layering (theme picker > fullscreen > help > globals > screen) has no key leaks; Esc semantics are consistent; mouse routing mirrors the same layering (asymmetries are intentional click-to-dismiss).

### 4.3 Drift & dead surface

- **13 of 40 Action variants are dead** (no producer or no consumer outside `action.rs`): `FocusNext, FocusPrev, OpenSearch, CloseSearch, SearchInput, SelectEffect, ApplyPreset, ScrollUp, ScrollDown, PageUp, PageDown, ScrollToTop, ScrollToBottom`. Delete them; views handle these concerns internally.
- **`ResetControls` has a handler but no producer** (`app.rs:841`) — controls reset is unreachable from the keyboard. Wire it (e.g. `r` in the controls pane) or drop it.
- Help overlay (`app.rs:1292-1321`) is hand-maintained and already drifted: Tab pane-cycling exists but isn't listed; `/`-search is listed globally but only works in the browser. Bindings and help should come from one table.

## 5. Architecture Judo

Ranked by leverage:

1. **Adopt `hypercolor-types` directly for scene/zone state.** The crate already depends on it; deserializing `Zone`/enums directly (with a thin render projection) avoids growing a third hand-mirrored DTO layer like the web UI's (doc 57 §5 Move 4 learned this the hard way).
2. **`App::notify(msg, level)` helper.** The `Some((Notification {…}, Instant::now()))` ritual appears ~10×, plus the same shape inside `spawn_actions`' error arm. ~50 lines deleted, one place to change toast policy.
3. **Delete the 13 dead Action variants + dead-action arms.** Shrinks the routing surface the broadcast list (below) has to reason about.
4. **Apply-flow slimming (pairs with Perf H1).** `refresh_effects_and_status` disappears; apply success = status + active-scene refetch. The two spawn helpers (`spawn_actions`/`spawn_command`, `app.rs:1176-1204`) are adequate once call sites shrink — a bigger `spawn_mutation` framework is not warranted at 6 call sites.
5. **One bindings table.** `const GLOBAL_BINDINGS: &[(key, label, Action)]` consumed by both `handle_key_event` and `render_help` kills the drift in §4.3.
6. **Broadcast match list** (`app.rs:998-1031`): fragile-by-hand but short; keep, with the new scene/zone actions added. Re-evaluate only if it grows past ~25 variants.
7. **Test inversion.** Core coordinators (`app.rs` 1490 lines, `bridge.rs` 428) have zero direct tests while leaf widgets are well covered; `tests/` files import the public API cleanly (no `#[path]` hacks — better than the web UI). New scene/zone bridge logic should land with event→action mapping tests.
8. **Raw `Color::Rgb` sprawl**: views define local palette consts (`dashboard.rs:23-31`, `effect_browser.rs:28-34`) while `theme.rs` exists; `app.rs` help/fullscreen hardcode 12 more. Consolidate into `theme.rs` opportunistically.

## 6. Executive summary & sequencing

**The headline:** the TUI is a well-built single-effect instrument — clean Action architecture, zero-copy frame path, solid preview pipeline — that simply predates zones and scenes. Its REST client has no scene/zone surface at all, applies silently retarget the primary zone, and `render_group_changed` events are dropped on the floor. Separately: every apply costs ~2N HTTP requests with an N-effect library (the single biggest perf win), global hotkeys eat half the alphabet while the user is typing in search (`q` quits), and a third of the Action enum is dead code.

| Phase | Work | Source |
|---|---|---|
| **0 — Correctness (hours)** | `captures_input` input-mode fix; delete dead Action variants; wire or drop `ResetControls` | §4 |
| **1 — Scene/zone data layer** | State types from `hypercolor-types`; REST: scenes/active-scene/activate/deactivate/zone-update/layer-controls + `render_group` on apply; new actions; bridge handling for `active_scene_changed` + `render_group_changed`; bootstrap fetches | §1.2, §1.3 |
| **2 — Apply-flow perf** | Apply/preset refresh = status + active scene (kill double N+1); event-driven effects refetch only for rescan/install | §3 H1 |
| **3 — Scene & zone UX** | Scene picker modal (+ Default row, lock glyph); zone focus cycling + visible apply target; per-zone Now Playing rows on dashboard; zone-aware status bar + fullscreen info bar; zone-scoped control editing | §1.3, §2 |
| **4 — Polish** | notify() helper; bindings table = help source; theme const consolidation; M1–M3 clone trims; bridge/app tests for new flows | §3, §5 |

Review performed by three parallel agents + verification pass on 2026-06-09; this doc is the canonical record.
