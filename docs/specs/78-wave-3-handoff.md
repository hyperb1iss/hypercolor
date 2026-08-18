# Spec 78 wave 78.3 — handoff

**Status at handoff:** 78.3a merged-ready (PR #210, gates green), 78.3b code-complete
and gating, 78.3c **not started**. This file is the cold-pickup brief for whoever
takes the wave next. Delete it when 78.3c lands.

**Spec:** `docs/specs/78-api-resource-model.md`, LOCKED rev 8. Wave 78.3 is
§8's third bullet; the contracts it mounts are §1, §2.3, §5.7, and the WS items
are §7.1's "78.3 owns" list.

---

## 1. The shape of the wave

78.3 is a three-PR stack, not one PR. The split exists so every intermediate
state is green and so the route flip lands last, with all clients moving in the
same PR as the deletion that forces them.

| PR | Branch | What it does | State |
| --- | --- | --- | --- |
| 78.3a | `nova/s78-w3a-scene-mount` | Mounts `/api/v1/scene` additively. No deletions, no client changes. | **PR #210**, `just verify` green (339 suites, 0 failed), adversarial pass done, 9 findings fixed |
| 78.3b | `nova/s78-w3b-ws-vocabulary` | Every §7.1 item 78.3 owns, plus the pre-staged paused reconciliation. | 6 commits, daemon + python + UI green, full `just verify` and adversarial pass in flight |
| 78.3c | not created | Deletes the singleton REST vocabulary, rewrites apply as sugar, migrates every client, rewrites the docs. | **not started** |

Both existing branches are stacked: `78.3b` is based on `78.3a`, which is based
on `origin/main` at `f0402402`. Rebase 78.3c on 78.3b.

### Worktrees

- `~/dev/worktrees/hypercolor/nova/s78-w3-scene-tree` — holds branch
  `nova/s78-w3a-scene-mount`. Named for the wave rather than the branch because
  it was created before the split; harmless, but do not be surprised by it.
- `~/dev/worktrees/hypercolor/nova/s78-w3b-ws-vocabulary` — holds branch
  `nova/s78-w3b-ws-vocabulary`.

Both have warm target directories and built `effects/hypercolor/*.html`. A third
worktree for 78.3c costs one `just effects-build` plus a cold workspace build;
budget roughly ten minutes before the first useful gate.

---

## 2. What 78.3a landed (context you need for 78.3c)

`/api/v1/scene` is mounted over `SceneMutation`/`commit_scene`. The adapter is
`crates/hypercolor-daemon/src/api/scene.rs`; the projection and the services the
pre-existing zone/layer services did not cover live in
`crates/hypercolor-daemon/src/domain/scene_tree.rs`.

**The one token.** `revision` is the commit generation — `SceneCommit::revision()`,
which is the same number as `generation()` by construction
(`domain/commit.rs:20-22`). It is **not** `groups_revision`, `layers_version`, or
`controls_version`. Structural writes check it inside the candidate via
`scene_tree::check_scene_revision(&mutation, expected)` against
`mutation.base_revision()`, so there is no TOCTOU window.

**Both tokens coexist during the transition.** `domain::zone`'s command structs
and `domain::layer`'s functions gained an **additive** `expected_scene_revision`
alongside their legacy `expected_version`. Every legacy call site passes `None`.
**78.3c deletes the legacy parameter and its field when the routes that speak it
die** — that is a required cleanup, not optional.

**New `DomainError` variant.** `ControlBound { keys }` renders 409 with code
`control_bound` and `details.bound`. Added rather than reusing `Conflict` because
§1.6 names both the code and the payload. By contrast, §1.4's `layer_not_found`
is rendered as the canonical 404 `not_found` with message `layer not found: <id>`,
because §8's canonical-envelope rule outranks an inline code spelling. Keep that
asymmetry; it was a deliberate call.

**Known transitional gap that 78.3c must close.** Applying an effect still writes
a layer whose id is derived from the zone id
(`core/src/scene/mod.rs::replace_legacy_effect_layer_stack` →
`Zone::legacy_layer_id()` = `SceneLayerId::from_uuid(zone.id.0)`), so an apply can
repeat a layer id — the exact fake-layer convention this wave deletes. It could
not be fixed in 78.3a because the TUI (`client/rest.rs`) and the web UI
(`pages/effects/zone_controls.rs`) still address controls by passing a zone id as
a layer id. **The apply rewrite and the client migration must land in the same
PR**, which is 78.3c.

---

## 3. What 78.3b landed

Six commits on `nova/s78-w3b-ws-vocabulary`:

1. `fix(session)` — `reported_paused()` now returns `sleeping()`, so a
   destructive stop reads as paused on the WS hello, MCP `get_status`, and
   `hypercolor://state`, matching `GET /output`. **Event-vocabulary decision
   made and documented: a stop publishes no `Paused` event**, because
   `EffectStopped` already announces that gesture and minting a pause would make
   the two indistinguishable on the stream. Clients converge on the next snapshot
   read. This closes sibyl task `5498ce89`.
2. `feat(events)` — `EffectControlChanged` gains `zone_id` + `layer_id`.
   `HypercolorEvent` gains `strum::EnumDiscriminants`, and
   `event_vocabulary()` emits every wire name plus the two synthetics.
   `InputSourceChanged` is **deleted** (no producer anywhere; it squatted the
   spelling the live input-status event has to avoid).
3. `refactor(ws)!` — hello loses `effect` and `active_preset_id`;
   `zone_layout_preview`/`_clear` lose `scene_id` and are active-scene-only;
   WS error codes become `malformed_request` / `validation_error` / `forbidden`.
   Tray, desktop app, TUI and web UI all migrated in the same commit. The TUI
   lost `DaemonState.effect_name`/`.effect_id` entirely and now reads the focused
   (falling back to primary) zone, which also deleted its last `/effects/active`
   call.
4. `refactor(ws)!` — tag `0x01` zone count widened to `u16` (11-byte header),
   frames JSON encoding deleted, `screen_zones` gains its own config block
   (default fps 15, deliberately the cadence it effectively ran at while
   borrowing `screen_canvas`'s), preview transport default flipped to v2, and
   every topic declares a backpressure class. The classes were **made true**:
   `metrics`, `device_metrics` and `sensors` dropped silently and now emit the
   notice.
5. `feat(ws)` — `protocol/websocket-v1.json` becomes generated output. New
   emitter at `crates/hypercolor-daemon/src/api/ws/manifest.rs` + binary
   `hypercolor-ws-manifest`, recipes `just ws-manifest` / `just ws-manifest-check`,
   CI step in the `python-generated` job. Authored prose, frame layouts, config
   bounds and JSON message lists moved to
   `protocol/websocket-v1.descriptions.json`.
6. `docs(ws)` — `docs/content/api/websocket.md` retold: backpressure class table,
   a continuity section stating the no-replay contract, and every wire change
   above. The Python event stream carries the same contract in its docstring.

**Golden fixtures re-blessed deliberately:** `01-frames-all.hex` and
`01-frames-selected.hex`, one byte each (`zone_count u8` → `u16le`). The bless
env var takes fixture names or `all`, **not** `=1`:
`HYPERCOLOR_WS_GOLDEN_BLESS=01-frames-all,01-frames-selected`.

---

## 4. 78.3c — the remaining work

This is the largest PR of the three. Everything below is required; nothing here
has been started.

### 4a. Delete without alias (§2.2, §1.5, §9.8)

Routes, all registered in `crates/hypercolor-daemon/src/api/mod.rs`'s
`build_router`:

- `/effects/active`, `/effects/active/cover`, `/effects/active/controls`,
  `/effects/active/controls/{name}/binding`, `/effects/active/reset`
- `/effects/stop` (the gesture moves to `POST /scene/clear`, already mounted)
- `/effects/{id}/controls`
- `/effects/{id}/layout` (GET/PUT/DELETE) **plus the `effect-layouts.json` store
  and its three link response types**. Release note, no migration — the
  successor is `Scene.layout_id`, which 78.1 already shipped.
- `/scenes/active` (replaced by `GET /scene`), `/scenes/deactivate` (moved to
  `POST /scene/deactivate`, already mounted)
- `/scenes/{id}/zones/*` — all 14, plus `/scenes/{id}/unassigned-behavior`
- `/scenes/{id}/layers/broadcast-media`
- `/library/presets/{id}/apply` — a parallel implementation with divergent
  semantics; Library keeps preset CRUD, applying happens through the one door

Types die with their routes, including `ActiveEffectResponse`,
`PauseEffectResponse`/`ResumeEffectResponse` (check whether 78.2 already took
these), `StopEffectResponse`, `ResetControlsResponse`, the effect-layout link
types, and `api::zones`/`api::layers` request shapes that no surviving route uses.

**Watch the same-name import trap:** `hypercolor_types::api::effects` and
`hypercolor_types::api::scene` both define `ApplyEffectRequest`/`ApplyEffectResponse`.
The `effects` pair is deleted; the `scene` pair survives. A file that imports both
will compile against the wrong one silently.

**The deleted-route fence** (`renamed_routes_leave_nothing_behind` pattern) must
grow every deleted path with envelope assertions. The API-scoped 404 fallback
landed in 78.2 (`ba17b6f6`) and already guards these deletions behind the web UI.

### 4b. Apply becomes sugar (§2.3)

`POST /effects/{id}/apply` and `POST /effects/{id}/presets/{preset}/apply` take
`types::api::scene::ApplyEffectRequest` and return `ApplyEffectResponse`
(`{ zone: ZoneResource, transition, output: SideEffectOutcome }`).

Semantics: **replaces the target zone's layer stack with a single new layer
carrying a fresh `SceneLayerId`**. Zone omitted means the primary zone, created
if the scene has none. Ordering is validate → commit → wake. 78.3a already fixed
the ordering and already computes the wake outcome —
`domain::effect::EffectApplied.output` exists and is currently unread; wire it
into the response.

**Post-commit failure is a 200**, never an error envelope, with `applied: false`
plus a message. Repair goes through `PATCH /output`, never a blind re-apply,
because apply mints a fresh layer id and is deliberately not idempotent.

`transition` accepts exactly `{ "type": "cut" }`.

**Playlist advancement becomes stack replacement with NO power wake** — an
unattended sequencer must not un-pause output. The divergent helper dies with
`/library/presets/{id}/apply`.

This is also where the §1.4 gap from 78.3a closes: make the apply path mint
`SceneLayerId::new()` and clear the stack rather than calling
`replace_legacy_effect_layer_stack`.

### 4c. Clients, in the same PR set (route flip last)

- **Web UI** — kill the fake-layer synthesis at
  `pages/effects/zone_controls.rs`; Studio moves to `/scene`. The UI is
  **excluded from the workspace**: `cargo check --workspace` does not cover it.
  Check it with
  `./scripts/cargo-cache-build.sh cargo check --manifest-path crates/hypercolor-ui/Cargo.toml`
  and test with `just ui-test`.
- **TUI** — kill the fake-layer synthesis at `client/rest.rs` (around the
  `legacy_layer_id` comment on `ZoneSummary`). 78.3b already removed its
  singleton status reads and its `/effects/active` call.
- **CLI** — `effects stop` → scene clear; effect control patches → layer
  controls; the `effects layout` subtree is deleted; `scenes active`/`deactivate`
  → `/scene`.
- **Python** — the hand-written facade owns the doomed paths. Regenerate with
  `just python-generate`; gate with `just python-generate-check`. Note `/scene`
  is **not** in the OpenAPI catalog (78.1 deliberately withheld `ToSchema` from
  `types::api::scene` until §7.2's utoipa-axum catalog after 78.5), so the facade
  hand-writes those calls the way it already does for zones.

### 4d. Docs checklist (§8, verbatim)

`guide/first-session.md`, `guide/quick-start.md`, `api/cli.md`,
`contributing/debugging.md`, `studio/zone-api-and-concurrency.md`,
`troubleshooting/studio.md`, `effects/controls.md`. Also grep all of
`docs/content` for the doomed paths — the checklist is a floor, not a ceiling.
`api/rest.md`'s 147 hand-written blocks become generated in wave 3.3, so do the
minimum there.

78.3a deliberately deferred its own prose docs to this PR, because the pages that
describe zone and layer editing all describe routes 78.3c deletes. Rewrite them
once, here.

---

## 5. Working notes that will save you time

- **Gates.** `just verify` = boundary check + build-wrapper + cargo-gc + fmt +
  clippy + workspace tests + alloc contracts. It takes 15-25 minutes and
  **exceeds the 10-minute foreground command cap**, so run it with
  `run_in_background: true` writing to a log, then wait on a
  `until grep -q VERIFY_EXIT` loop.
- **`cargo check -p <crate> --all-targets` is not enough** before claiming a
  shared-type change compiles. Changing a `hypercolor-types` enum or a client
  struct breaks test targets in `hypercolor-types`, `hypercolor-tui`,
  `hypercolor-app` and `hypercolor-leptos-ext` that a daemon-scoped check never
  builds. Use `cargo check --workspace --all-targets`, then `just lint`, then
  the full gate. 78.3c touches every client, so this will bite otherwise.
- **The web UI is outside all of that.** `--workspace` excludes
  `crates/hypercolor-ui` entirely; check and test it on its own manifest.
- **`just effects-build` once per worktree** before workspace tests.
- **Known flakes, not your bug.** `pipeline_gpu_*` in
  `crates/hypercolor-daemon/tests/render_thread_tests.rs` fail under parallel
  load and pass serialized (`-- --test-threads=1`, 46/46). Sibyl task
  `be5c6712` tracks the root cause. `release_sleep` is Windows-skipped.
- **`ControlValue` is externally tagged.** A control patch body is
  `{"values": {"speed": {"float": 0.5}}}`, never `{"speed": 0.5}`. Three tests
  failed with a 400 before this was spotted.
- **`DomainError::Validation` maps to 422**, not 400, in this codebase.
- **Do not put `deny_unknown_fields` on query structs** — `rest_v1_compat_tests`
  pins that `/effects?offset=2&limit=1` silently discards paging args.
- **Sibyl** has the full 78.3a decision record at `decision_fa943db33b1b`
  (pinned), including all nine adversarial findings and why each mattered.

---

## 6. Verification contract

Every PR in this wave gets an independent adversarial verifier briefed to
**refute**, not confirm. It cannot message you; collect from its final report,
fix, re-gate, and include the verdict in the PR body. The 78.3a pass returned
FAIL with nine findings — a zone patch that ignored `If-Match`, a control patch
that skipped schema validation, a clear gesture that blanked display faces, a
`GET` that committed — none of which the implementer's own checks had caught.
Budget for a fix round; do not treat a clean self-check as a substitute.
