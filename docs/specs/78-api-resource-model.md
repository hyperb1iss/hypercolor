# Spec 78: Canonical API Resource Model

**Status:** LOCKED rev 7 (2026-08-17) — four codex review rounds (finding trajectory 20 → 8 → 1 → 0), one post-convergence coordination update, and one owner reconciliation pass against the 2026-08-17 merge sweep (ten PRs landed between rev 5 and lock (#195-#204: six Spec 76 waves, four CI/build fixes); every §7 amendment re-verified against main, dispositions in §10).
**Findings base:** `docs/review/api-surface-review-2026-08-16.md` — full-surface review (REST route table read + four audit lanes: WS, MCP, clients, OpenAPI/security/docs) with file:line receipts, verified against the tree on 2026-08-16. Local artifact (`docs/review/` is gitignored); re-check line numbers at execution time.
**Relationship to Spec 76:** Spec 76 unifies the *internals* (domain services, typed contracts, envelope, WS registry, OpenAPI catalog). This spec redesigns the *surface* those mechanics serve: the resource model, the route set, and the naming. It amends Spec 76 where the two touch (§7) and subsumes wave C1b (§7.3).
**Authorship model:** same as Spec 76 — Fable owns the contracts and writes the contract-bearing code first; Opus 5 workers execute mechanical waves; Codex reviews per PR.

---

## 0. Goals, non-goals, doctrine

**Goals**

1. One model on the wire: the scene tree (active scene → zones → layer stacks → controls) is the only mutation vocabulary for what renders. The pre-multi-zone singleton vocabulary is deleted, not aliased.
2. A surface a human can hold in their head: 80 paths (from 111), one name per concept, one request shape per operation class, verbs with exactly one meaning each. Appendix A is the normative inventory.
3. Convenience without parallelism: high-traffic gestures (apply an effect, stop the show, set brightness) stay one call, defined as documented sugar over the canonical model and returning canonical resources.
4. Significantly less code: dead routes, dead types, second implementations, and hand-mirrored shapes deleted across daemon and all in-repo clients.

**Non-goals:** no rendering behavior changes; no change to the domain service layer's design (Spec 76 §2 stands); no WS frame-layout redesign beyond the named wire fixes in §7.1; no new features beyond the affordances in §5 (which exist to delete client-side workarounds, not to grow scope).

**Doctrine:** lockstep, verbatim from Spec 76 §0. Pre-1.0, single-user, in-repo clients ship in lockstep, generated clients regenerate. Every route deletion or rename lands with all in-repo consumers updated in the same PR — *consumers* includes the hand-written docs pages named per wave in §8, not just code. Pinned-shape tests are rewritten in the same PR. Persisted user state migrates forward once (§3.3) and is never destroyed.

---

## 1. The scene tree — `/scene`

### 1.1 Invariant

An active scene always exists: the auto-managed default scene cannot be renamed or deleted (`core/src/scene/mod.rs:195,255,273`), and deactivation returns to it. Therefore `GET /scene` always returns 200. There is no idle sentinel, no 404-vs-null convention split, and no all-optional response shape anywhere in this tree.

### 1.2 Routes

```text
GET    /scene                                  full live document (1.3)
PATCH  /scene                                  scene-level fields: name (non-default only), unassigned_behavior
POST   /scene/deactivate                       return to the default scene; responds with the new /scene document
POST   /scene/clear            {zone?}         clear one zone's layer stack, or every zone's; the "stop" gesture

POST   /scene/zones                            create zone
GET    /scene/zones/{zone}                     zone resource (embedded in /scene; addressable for follow-ups)
PATCH  /scene/zones/{zone}                     name, structural fields
DELETE /scene/zones/{zone}
PUT    /scene/zones/{zone}/layout              zone-scoped spatial layout override
POST   /scene/zones/{zone}/members             assign device segments to the zone
DELETE /scene/zones/{zone}/members/{member}    unassign one membership

GET    /scene/zones/{zone}/layers
POST   /scene/zones/{zone}/layers
PATCH  /scene/zones/{zone}/layers/order
PUT    /scene/zones/{zone}/layers/{layer}
DELETE /scene/zones/{zone}/layers/{layer}
PATCH  /scene/zones/{zone}/layers/{layer}/controls
```

All mutations route through `SceneMutation`/`commit_scene` (Spec 76 §2.3) — `/scene` is the REST adapter for the live tree, nothing else.

**Members, not device-zones.** Zone membership is addressed by the membership's own globally unique id (today's layout `Output.id`), returned in the zone document — never by a device-scoped segment name, which is not unique across devices (`spatial.rs:337-351`). The request body names a device and its segments; the resource identity is the membership id.

### 1.3 The `/scene` document embeds layer identity — this kills the fake-layer hack

`GET /scene` returns the scene metadata (`id`, `name`, `kind`, `is_default`, `unassigned_behavior`, `layout_id`, `revision`) and every **authored zone** with its full layer stack: layer ids, effect refs, control values, blend/opacity. Clients never synthesize ids: the "synthetic legacy layer" convention (zone id passed as both zone and layer id — `zone_id` post-C1b; receipts `tui/src/client/rest.rs:361-366`, `ui/src/pages/effects/zone_controls.rs:255` from the pre-C1b tree) is deleted along with the routes it forged. A client patches the real layer id it read from `/scene` (or from the apply response, §2.3). No zone-scoped controls route exists — conditional shorthand routes ("valid only when the stack has one layer") are rejected as a class.

**Display faces stay display-domain.** Runtime default display faces materialize as `default_display_groups` outside the authored scene (`core/src/scene/mod.rs:1893-1934`), and their controls live in display preferences. They are deliberately NOT projected into `/scene`: display composition is owned by `/displays/{id}/face` + `face/controls` + `face/composition`, which this spec retains unchanged. One owner per pixel path: authored zones belong to `/scene`, faces belong to `/displays`.

### 1.4 Layer identity lifecycle

- Every layer id is a `SceneLayerId` UUID minted at layer creation. Replacement is creation: **every** successful whole-layer `PUT .../layers/{layer}` and every apply (§2.3) mints a new layer id, same effect or not — an id never survives replacement. In-place mutation is exactly two operations: `PATCH .../controls` and `PATCH .../layers/order`.
- A control patch addressing a vanished layer returns 404 `layer_not_found`; the client recovers by re-reading `/scene` (the UI stays current via WS events). Stale patches can never land on a newer effect.
- Layer and zone ids in **persisted** scenes are stable across activation, deactivation, daemon restart, and snapshot. Ids inside the auto-managed default scene are stable within a daemon run only; clients must not persist them.

### 1.5 Fine-grained mutation is live-tree-only

`/scenes/{id}/zones/*` (14 routes; the `groups` spelling died in C1b) are deleted. Editing a stored scene is either whole-document (`PUT /scenes/{id}`) or live (activate it, edit `/scene`, changes persist through the existing commit path). This matches how the product is used — Studio edits the live scene against real hardware — and removes the doubly-parameterized tree whose paths carried two different "zone" concepts at once. The WS `zone_layout_preview` command loses its caller-selected `scene_id` and becomes active-scene-only, keyed by `zone_id` (§7.1) — the live-tree invariant holds across transports, not just REST.

### 1.6 Concurrency — one token

The scene document carries one wire version: `revision`, the commit generation assigned by the sequencer (Spec 76 §2.3). `GET /scene` serves it as `ETag`; **structural** mutations (scene-level `PATCH /scene`, zone create/delete/patch, zone `PUT .../layout`, member assign/unassign, layer create/delete/reorder/replace, clear, `PUT /scenes/{id}`, and both apply sugars — `/effects/{id}/apply`, `/effects/{id}/presets/{preset}/apply`) honor optional `If-Match` against it and return the canonical 412 envelope (`details: { current }`) on mismatch. The three-token zoo (`groups_revision`, `layers_version`, `controls_version`) disappears from the wire; internals keep whatever bookkeeping Spec 76 §6.5 needs.

**Control-value writes take no token.** `PATCH .../layers/{layer}/controls` is unguarded: the server applies value writes in commit order, and the layer-id lifecycle (§1.4) already fences staleness — a replaced layer 404s rather than absorbing a stale patch. (The guarded form was unusable in practice: a guarded slider drag self-invalidates every tick, which is why the TUI ships `If-Match`-free today, `tui/src/client/rest.rs:348-378`.) A PATCH naming a control key with an active input binding is rejected 409 `control_bound`, listing the bound keys — a manual write that would be silently overridden by the next sensor resolution is an error, not a race. The rejection is recoverable in the same shape: `PatchControlsRequest.clear_bindings` (§5.7) removes named bindings and applies the accompanying values in one atomic commit. Persisted bindings therefore stay fully manageable — surfaced read-only in the layer document, removable via `clear_bindings` — while the standalone binding-creation route dies until bindings return as a first-class feature (§2.2).

---

## 2. Effects become a catalog; one sugar verb survives

### 2.1 Catalog routes

```text
GET    /effects                ?category&audio_reactive&screen_reactive&input_reactive&q&include=controls,presets
GET    /effects/{id}
GET    /effects/{id}/cover
GET    /effects/{id}/presets
POST   /effects/{id}/presets/{preset}/apply    sugar; delegates to 2.3
POST   /effects/{id}/apply                     sugar; see 2.3
POST   /effects/install
POST   /effects/rescan
```

The filters are implemented server-side, once, in the effect service. Today the handler takes no `Query` extractor at all (`api/effects.rs:352`), which makes the CLI's filter flags silently dead, forces the UI to filter the catalog in WASM, and left MCP to grow a third private filter implementation. `include=controls,presets` expands summaries in the list response, deleting the TUI's N+1 catalog hydration (`tui/src/client/rest.rs:93-127`).

### 2.2 Deleted from the effects domain

`/effects/active`, `/effects/active/cover`, `/effects/active/controls` (C1b's renamed spelling of `current`), `/effects/active/controls/{name}/binding`, `/effects/active/reset`, `/effects/{id}/layout` (GET/PUT/DELETE — the effect-to-layout link, plus its `effect-layouts.json` store and the three link response types; §9 decision 8), `/effects/pause`, `/effects/resume`, `/effects/stop`, `/effects/{id}/controls`, and the `/effects/screenshots` static mount. Live state lives at `/scene`; pause lives at `/output` (§4); stop is `/scene/clear`; control patches address real layers (§1.3). Binding *creation* returns with a first-class binding design; until then bindings are surfaced in the layer document and removable via `PatchControlsRequest.clear_bindings` (§1.6) — never stranded. `PauseEffectResponse`/`ResumeEffectResponse`/`ActiveEffectResponse` and the idle-sentinel shape die with the routes.

### 2.3 The sugar contract

`POST /effects/{id}/apply` body: `{ zone?, controls?, preset_id?, transition? }` (`render_group` renamed `zone`; `controls` typed `BTreeMap<String, ControlValue>` per Spec 76 §4.5, never `serde_json::Value`).

Semantics, stated in the OpenAPI description verbatim: *replaces the target zone's layer stack with a single new layer (fresh `SceneLayerId`, §1.4) running this effect* (zone omitted = primary zone, created if the scene has none). It is a projection of the same `SceneMutation` a layer-stack replacement performs — not a second code path.

**Side effects are part of the contract, in this order:** (1) validate effect, zone, controls — any failure returns before any state changes (today power wakes before validation, `domain/effect.rs:190-191`; this spec fixes that ordering as part of the wave); (2) commit the scene mutation; (3) wake paused output. Response: the updated **zone resource** (canonical §1.3 shape, carrying the new layer's id) and the applied transition. `ApplyEffectResponse`'s bespoke shape is deleted. (Rev 6 had a step 4 resolving the effect's linked layout; rev 7 deletes the link mechanism itself, §9 decision 8, which also removes the layout outcome from this response.)

**Post-commit failure is a 200, and repair is targeted.** Once step 2 commits, the response is `200` with the outcome fields reporting any step-3/4 failure (`applied: false` plus a message) — never an error envelope, because the resource state the response describes is real. Error envelopes are reserved for step-1 refusals, where nothing changed. Clients repair a failed side effect through that side effect's own route (`PATCH /output`, `POST /layouts/{id}/apply`) rather than re-applying: a retried apply is not idempotent (it mints a fresh layer id, §1.4). The same rule governs activation's layout/brightness outcomes (§3.2).

**Transitions are honest:** rev 1 accepts exactly `{ type: "cut" }` — the only transition the engine performs today (`domain/effect.rs:24-85` rejects everything else). The type is a closed enum that grows when the engine does; the request field does not accept aspirational values.

**Preset apply is effect-scoped only.** `POST /effects/{id}/presets/{preset}/apply` delegates to this contract (it already does, `api/effects.rs:531-539`). `POST /library/presets/{id}/apply` — a parallel implementation with divergent semantics (hot-swaps controls without apply's power/layout behavior, `api/library/presets.rs:223-340`) — is **deleted**. Library keeps preset CRUD; applying happens through the one door.

**Playlist advancement is stack replacement only.** Playlists drive the same zone-stack replacement with **no** power wake — an unattended sequencer must not un-pause output. (Rev 6 also exempted playlists from layout-link resolution; rev 7 deletes that mechanism everywhere.) Stated in the playlist service contract; the divergent helper dies with `/library/presets/{id}/apply`.

---

## 3. Scenes collection, snapshots, and the end of profiles

### 3.1 Collection routes

```text
GET    /scenes                 list
POST   /scenes                 create (seeds a Primary zone, as today — api/scenes.rs:137-149)
POST   /scenes/snapshot        create a scene from current runtime state (replaces profile-create)
GET    /scenes/{id}
PUT    /scenes/{id}            whole-document replace
DELETE /scenes/{id}
POST   /scenes/{id}/activate
```

`/scenes/active` is deleted (`GET /scene` replaces it); `/scenes/deactivate` moves to `/scene/deactivate`.

**`PUT /scenes/{id}` is whole-document replace, and the request says so.** The body is the full scene document — the same shape `GET /scenes/{id}` returns minus server-assigned fields (`revision`; ids for zones/layers the caller is creating may be omitted and are minted server-side, while supplied ids must belong to this scene). The path id is authoritative; a body `id` must match or the request is a 422. Omitted **optional** fields clear — replace means replace; a client that wants read-modify-write reads first. `If-Match` against `revision` per §1.6. Today's partial-update `UpdateSceneRequest` (`api/scenes.rs:97-263`) dies with this wave; partial semantics live on `PATCH /scene` (live tree) only.

### 3.2 Scenes gain what profiles actually carried

A profile is a pre-multi-zone snapshot: primary effect + controls + preset, display face assignments, a **named layout reference**, and a **brightness** (`profile_store.rs:25-54`). Folding profiles into scenes without losing capability means scenes own two new optional fields:

- **`layout_id: Option<LayoutId>`** — a scene may reference a named spatial layout; activation applies it (the `apply_layout_update` path profile-apply uses today, `api/profiles.rs:307-335`). A dangling reference is kept, skipped at activation with a warning event — never silently dropped. `POST /scenes/snapshot` captures the currently applied layout's id.
- **`activation_brightness: Option<f32>`** — applied to `/output` on activation when present. Snapshot does **not** capture it (brightness is global output state, §4); the field exists so migrated profiles keep their restore-brightness behavior and so a user can opt a scene into it explicitly via `PUT /scenes/{id}`. Migration converts the profile's `u8` percentage as `activation_brightness = f32::from(brightness) / 100.0`.

Display face assignments are representable today (Display-role zones; `scene.rs:81-119`), `active_preset_id` maps to the layer's preset ref (`layer.rs:168-176`), and `Profile.description` maps verbatim onto the scene description. With these two fields, the profile → scene mapping is lossless.

**Activation ordering is part of the contract:** (1) validate the scene exists and commit the switch; (2) apply `layout_id`, if present; (3) apply `activation_brightness`, if present. Steps 2 and 3 run after the switch and never roll it back; their outcomes are reported in the activation response (`{ layout: { layout_id, applied }, brightness: { applied } }`) under the §2.3 post-commit rule — a failure after the switch is visible, not swallowed and not a lie of atomicity.

All six `/profiles/*` routes, the profile store, `ProfileId`, MCP `set_profile`, and the `hypercolor://profiles` resource are deleted. The `daemon.start_profile` config key becomes `start_scene` (schema bump + rename in normalize; release note). MCP gains `hypercolor://scenes` (§6.7).

### 3.3 Migration — an import, not a path move

The Spec 76 §3.4 harness moves one store between paths; it cannot merge two stores (`path_migration.rs:380-417`), so profiles do not go through it. Instead, a one-time **import** on first startup of the shipping version:

1. Read `profiles.json`. Convert each profile to a named scene: primary → Primary-zone layer (effect + controls + preset ref), displays → Display-role zones, `description` verbatim, `layout_id` and `brightness` → the §3.2 fields.
2. Each imported scene's id derives deterministically from its profile id (UUIDv5 over a fixed import namespace), which makes the import an **idempotent upsert**: a crash between the durable scene write and source retirement re-runs the import as a no-op overwrite, never a duplicate. Name collisions take the first free numbered suffix (`name (imported)`, `name (imported 2)`, …) computed across existing and staged scenes, **excluding the import's own destination id** — an upsert overwriting the scene it wrote on a previous run keeps that scene's persisted name rather than re-suffixing it. Superseded writes follow the store's retry semantics.
3. Retire `profiles.json` (harness-style backup rename) only after the canonical scene write is durable. A conversion that cannot represent a field **fails the import and leaves the source untouched** — with §3.2 the representable set is total, so this is a backstop, not an expected path.

Nothing dual-reads profiles at runtime after the importing run.

---

## 4. One output resource

```text
GET    /output                 { power: "running" | "paused", brightness: f32 }
PATCH  /output                 partial: either or both fields
```

Receipts for the merge: `POST /effects/pause` is literally `set_output_power(OutputPowerMode::Paused)` (`api/effects.rs:944-945`); no first-party client calls pause/resume (the UI already PUTs `/output/power` directly); and brightness's separate home is the last surface trace of the three-home problem Spec 76 §6.6 collapses. Deleted: `/output/power` (renamed onto `/output`), `/settings/brightness`, and with it the entire `settings` domain — `GET /audio/devices` moves to `/system/audio-devices` where it always belonged. MCP `set_brightness` and `set_output_power` become projections of the one output service; `set_brightness`'s phantom `device_id`/`transition_ms` params are deleted (§6.1).

---

## 5. Naming and shape conventions (normative for every route)

1. **Segments.** Device LED regions are `segments` in every path, parameter, and `types::api` field; `zones` is reserved for scene render zones (Spec 76 §4.4). Renames: `/devices/{id}/zones/{zone_id}/identify` → `/devices/{id}/segments/{segment}/identify` (`{segment}` is device-scoped here — the device id is in the path). Zone membership is addressed by membership id, not segment name (§1.2). Internal core types rename in a scheduled mechanical wave (§8.5).
2. **Frames are `/frame`; previews are tryouts; dry-runs are `validate_only`.** `GET /displays/{id}/preview.jpg` → `GET /displays/{id}/frame` (same JPEG body; simulators already use `/frame`). `POST /devices/{id}/attachments/preview` is deleted; `PUT /devices/{id}/attachments` accepts `validate_only: true` and returns the computed result without committing. `PUT /layouts/active/preview` keeps its name — a genuine live tryout.
3. **`deactivate` clears an exclusive current; `clear` empties a stack.** `POST /library/playlists/stop` → `/library/playlists/deactivate`. `stop` disappears from the API vocabulary.
4. **`PUT /capture/source`** replaces `POST /capture/source/pick` — it sets state, so it says so.
5. **Sum types are tagged enums.** A response meaning one-of-N shapes is an internally-tagged serde enum in `types::api`. Nullable-field flattening with a string sentinel (the old `ActiveEffectResponse`) is review-rejected as a class.
6. **Closed vocabularies are enums.** `EffectCategory`, effect `source`, transition types, power state serialize as typed enums with `ToSchema`, not `String`. (Receipt for the cost of strings: MCP's hand-typed category list fabricated three categories and omitted four real ones, `mcp/tools/effects.rs:67` vs `types/effect.rs:57-84`.)
7. **One control-patch shape.** `PatchControlsRequest { values: BTreeMap<String, ControlValue>, clear_bindings: Vec<String> }` in `types::api`, used verbatim at every scope: layer controls, display face controls, control-surface values. `clear_bindings` is meaningful only where bindings exist (layers); other scopes reject a non-empty list with a validation error. Today those are four hand-rolled shapes.
8. **Concurrency per §1.6:** one scene `revision` token, structural writes optionally guarded, value writes unguarded with `control_bound` rejection. No other wire version tokens exist.
9. **Attachments read in one call.** `GET /devices?include=attachments` embeds each device's attachment profile in the list response, deleting the UI's one-GET-per-device serial loop (`ui/src/components/layout_builder/editor_session.rs:76-83`). Per-device routes remain for writes.

---

## 6. MCP realignment

MCP stays a curated concierge, but every tool becomes an honest, thin projection of a domain service:

1. **Phantom parameters are deleted** (a parameter exists only when its behavior does): `set_effect.devices`, `set_color.devices` (hardcode `target_zone: None` while advertising targeting, `tools/effects.rs:233,477`), `set_brightness.device_id` (reports `"scope": "device"` while setting global) and `.transition_ms`, `diagnose.device_id`/`.checks` (handler binds `_params`), `stop_effect.transition_ms`, and `set_effect.transition_ms` — its `maximum: 0` form honestly rejected nonzero values, but a parameter accepting only its no-op value fails the same rule. `set_effect` gains the §2.3 `transition` closed enum instead (rev 1: `cut` only), so MCP and the REST sugar share one transition vocabulary that grows when the engine does.
2. **`create_scene` routes through the domain scene service** — same Primary-zone seeding and `publish_scene_library_changed` as REST. Its fictional `trigger`/`profile_id` params (written to metadata nothing reads) are deleted, and the `setup_automation` prompt stops promising automation the daemon does not have.
3. **The second diagnostics engine is deleted** (`tools/system.rs:380-555`): the tool calls the same domain diagnostics REST uses, with one check vocabulary.
4. **One resolver policy.** Fuzzy resolution returns a structured ambiguity error listing candidates (the pattern `set_display_face` already implements) instead of first-substring-hit over a `HashMap`.
5. **Tools and resources share payload builders.** `get_status` / `hypercolor://state` and `get_devices` / `hypercolor://devices` render from one builder each; the drifted duplicates die. The dead stub readers and their validators (`resources.rs:109-182`) are deleted with their scaffolding tests.
6. **New tool: `adjust_controls`** — patch named control values on a zone's layer (resolves zone → layer via the scene service). The one missing agent affordance: today an agent must re-apply an effect to nudge a slider.
7. **Renames tracking the model:** `set_profile` deleted (`activate_scene` covers it; a `snapshot_scene` tool ships if agent demand appears); `stop_effect` becomes `clear_zone` over `/scene/clear`; `hypercolor://profiles` becomes `hypercolor://scenes`; the `setup_automation`/`mood_lighting` prompts and the MCP docs pages update in the same wave (`mcp/prompts.rs:228-264`, `docs/content/agents/*`, `docs/content/api/mcp.md`).
8. **Honest annotations:** per-tool `destructive`/`read_only` hints (today `destructive(false)` is hardcoded for all tools including one whose description begins "Destructively stop").

---

## 7. Amendments to Spec 76 waves

### 7.1 WS registry — the 3.2 arc is COMPLETE; the residue transfers here

Spec 76's 3.2 arc landed in full before this lock (registry contract PR #193, wire-stable daemon adoption #196, keyed wire migration #203). Rev 5 wrote this section as amendments to 3.2's acceptance criteria; that ship has sailed, so each item below is now either **struck as landed** (with the PR that shipped it) or **owned by a Spec 78 wave**. Re-verified against main at lock time:

**Landed — struck from the workload:**
- The zone-vocabulary renames (`group_id`/`group_name`, `RenderGroupChanged`, WS message fields) shipped across C1b (#195) and 3.2c (#203); the 3.2c verifier's sweep confirms zero group-vocabulary survivors on any WS path. `groups_revision` inside persisted scenes remains the deliberate carve-out.
- RPC tags `0x80`/`0x81` and their codec were deleted by wave C1e (#191).

**78.3 owns (verified still open at lock):**
- Drop the hello payload's singleton `effect`/`active_preset_id` fields (`active_preset_id` confirmed live at `ws/protocol.rs:912`) — both UIs consume `/scene` + events instead. Give `EffectControlChanged` zone + layer identity.
- `zone_layout_preview` becomes active-scene-only and zone-keyed; the caller-selected `scene_id` field (confirmed live at `ws/protocol.rs:523,528`) is deleted.
- Every `define_ws_topics!` entry declares a **backpressure class** — `Lossless` (awaited send), `LatestWins` (watch semantics), or `DropWithNotice` (try-send + `backpressure` message) — replacing today's ad-hoc behaviors; the manifest states each topic's class.
- The **event vocabulary is generated**: the manifest's event list is emitted from `HypercolorEvent` plus the synthetic names (`resync_required`; `input_source_status_changed` renamed to not collide with `input_source_changed`).
- **One error vocabulary:** WS-native errors serve `ApiErrorDetail` codes; the parallel code set is deleted.
- **Named wire fixes:** `0x01` gains a `u16` zone count (u8 truncates at 255 zones silently); `screen_zones` gets its own config block (confirmed still configless in the registry at lock — it is paced by an *unsubscribed* channel's config); the default preview transport becomes the advertised v2 (verify at wave start; 3.2c staged negotiation but the default was out of its scope); the `frames` JSON toggle is deleted (still live: `FrameFormat::Json` at `leptos-ext/src/ws/registry.rs:125`, JSON frame building at `ws/cache.rs:339`; zero consumers).
- **The manifest becomes generated output** from the topic registry and codec metadata (the generator at `python/scripts/generate_ws_protocol.py` still consumes the hand-maintained `protocol/websocket-v1.json`); hand-editing becomes impossible by construction. The golden suite's source-scanning tag census (shipped in 3.2) is the pattern to extend.

**78.4 owns:** drop the hello `profile` field and delete the profile events (`event.rs:849-864`) — only 78.4, so no intermediate revision has profile REST without profile WS. Golden fixtures update in the same PRs.

**Documented contract:** the events channel does not replay across a socket gap; clients fold `connection_generation` into fetch epochs (the UI already does). Writing this into the manifest and SDK is 78.3's job — neither states it today.

### 7.2 OpenAPI (amends wave 3.3)

Wave 3.3's registration-helper catalog is implemented with `utoipa-axum`'s `OpenApiRouter` (utoipa 5.4 is already a workspace dep): route registration *is* spec registration, so drift is impossible rather than detectable. Acceptance criteria beyond Spec 76 §4.6: response schemas on every operation (today: zero catalog operations have any), true status codes (401/403/429 from security, 201/202 from the 17 handler sites, 101 for `/ws`), typed path parameters, and an extension hook so `ApiExtension` implementors contribute paths. The docs site's endpoint reference (`docs/content/api/rest.md`, 147 hand-written blocks) is generated from the emitted document; conceptual pages stay hand-written.

### 7.3 Wave C1b is consumed — it landed, and 78 deletes what it renamed

C1b landed as PR #195 (2026-08-17): `current`→`active` route renames, `groups`→`zones` in layer paths and zone payload fields, the display-face field rename with its shared-fixture fence. Its *vocabulary* renames are load-bearing groundwork this spec builds on; its renamed *routes* are transitional — wave 78.3 deletes `/effects/active-formerly-current/*` and the `/scenes/{id}` fine-grained tree wholesale (§2.2, §1.5). The affected client call sites flip twice (rename, then move to `/scene`); accepted and named here. Config resource routes were wave 4.3's job and landed (#190/#184).

**Wave 3.1 status at lock — the rev-5 gating was overtaken by execution.** Rev 5 gated the `devices+scenes` batch behind 78.3/78.4/78.5 and `effects+library` behind 78.3/78.5; both dispatched before this spec was read into the program:

- `layouts+displays+assets` landed (#202) — unaffected, as rev 5 predicted.
- `devices+scenes` landed (#204) **ahead of its gate**. Accepted churn, named here: 78.3/78.4 will delete or reshape some of its contracts (the scene activate/deactivate acknowledgements gain the §3.2 outcome fields; the fine-grained zone-route responses die with their routes). The promotion still pays for itself — the reshape now propagates by compiler across every client instead of by hand-hunting mirrors, which is the exact failure C1b demonstrated.
- `effects+library` was rescoped mid-flight to **Appendix A survivors only** (no promotions for `/effects/active|current/*`, pause/resume/stop acknowledgements, or `/library/presets/{id}/apply`); the template-catalog drift fix stays in.
- `drivers+system` remains unaffected except for the §5 renames and may run before or after 78's waves.

The standing rule survives the overtaking: 3.1 batches **extend** the 78.1 contracts and never redefine a type 78.1 ships.

---

## 8. Implementation plan

Waves are atomic PRs from lane worktrees, every in-repo consumer updated in-PR (code and the docs pages each wave names), pins rewritten in-PR, `just verify` green, Codex inline review, Fable signoff.

**Error rendering rule for every wave:** every route this spec adds or rewrites renders the canonical `DomainError` envelope from its first commit. Spec 76 waves 2.1 and C1a are both landed (C1a merged 2026-08-16, PR #192), so the canonical envelope is the only error surface this spec ever touches; no new route may reintroduce a bespoke shape.

**Route-inventory strategy:** Spec 76 wave 0.7's REST matrix remains the sole executable current-state pin, updated in every route-changing PR. Wave 78.1 additionally commits Appendix A as a **target manifest** (data, not a live assertion). A convergence test asserting live router ≡ Appendix A (Appendix A's scope: `/api/v1` JSON routes + `/health`; document routes excluded) activates at the end of wave 78.5 (the config-route rows landed with Spec 76 wave 4.3).

**78.0 — bug strikes** (independent, immediate; each lands regardless of the redesign)
- 0a. Server-side effect filters + `Query` extractor (fixes the CLI's dead flags; UI drops WASM filtering; MCP drops its private filter).
- 0b. MCP phantom-parameter deletion + honest annotations + category enum from strum + docs tool-count fix.
- 0c. TUI status shadow fix (render_loop fields; FPS shows again).
- 0d. Static assets and Swagger UI exempted from bearer auth; constant-time key comparison. (`/preview` was in this list at rev 5; wave 3.2c deleted the page outright, #203. The rest of the security findings route to Spec 77; these two are unambiguous now.)

**78.1 — contracts (Fable).** `/scene` document + zone/layer resource types (incl. layer-identity rules §1.4 and the single `revision` token §1.6), `/output` type, `PatchControlsRequest`, closed enums, tagged-union convention, segment naming and `zone` body fields in `types::api`, scene `layout_id`/`activation_brightness` fields, the §2.3 sugar contract, and Appendix A committed as the target manifest.

**78.2 — output merge (worker, after 78.1).** `/output` GET/PATCH; delete pause/resume routes + types, `/settings/*`, `/output/power`; move audio devices to `/system/audio-devices`; MCP brightness/power onto the output service. Clients in-PR.

**78.3 — the scene tree (worker(s), after 78.1; Spec 76 wave 2.3b landed).** Mount `/scene` over the domain services (validation-before-side-effects ordering per §2.3); delete `/effects/active/*` (C1b's renamed spelling), `/effects/stop`, `/effects/{id}/controls`, `/effects/{id}/layout` + the `effect-layouts.json` store (release note; no migration — the concept's successor is `Scene.layout_id`, which a user sets deliberately), `/scenes/active`, `/scenes/deactivate`, `/scenes/{id}/zones/*`, broadcast-media, `/library/presets/{id}/apply`; rewrite apply as sugar returning the zone resource; playlist advancement semantics; `/scene/clear`; the §7.1 items 78.3 owns (hello singletons, `EffectControlChanged` zone+layer identity, `zone_layout_preview` keying, backpressure classes, generated event vocabulary and manifest, error vocabulary, named wire fixes, and stating the no-replay continuity contract in the manifest and SDK). Both UIs and the CLI migrate in the same PR set (split by client if needed, route flip last). Docs checklist: `guide/first-session.md`, `guide/quick-start.md`, `api/cli.md`, `contributing/debugging.md`, `studio/zone-api-and-concurrency.md`, `troubleshooting/studio.md`, `effects/controls.md`.

**78.4 — profiles fold (worker, after 78.3).** `/scenes/snapshot`; the §3.3 import; scene `layout_id`/`activation_brightness` activation behavior; delete `/profiles/*`, store, types, MCP `set_profile` + `hypercolor://profiles` (add `hypercolor://scenes`); `start_profile` → `start_scene` config rename; CLI `profile` commands become `scene snapshot`/`scene activate`. Docs checklist: `guide/profiles-and-scenes.md`, `docs/content/agents/*` (prompt-templates, resources-reference, tools-reference, workflows), `api/mcp.md`.

**78.5 — naming + dead-route slash (worker).** Segments rename (API boundary), displays `/frame`, `validate_only`, `capture/source`, playlists `deactivate`; delete debug routes, bindings/rebind, `/diagnose/memory` (fold into diagnose checks), sensors item route, attachment vendors/categories/item-ops, `/devices/metrics`, `/effects/screenshots` mount, logical-devices (pending the downstream check below). Activate the Appendix A convergence test. Docs checklist: `api/openapi.md`, remaining pages the route grep names. Internal segment-type rename as its own mechanical PR.

**78.6 — MCP realignment (worker, after 78.3/78.4).** §6 items 2 to 8; `adjust_controls`; shared payload builders; dead stubs deleted.

**Downstream check (blocks only the logical-devices deletion in 78.5):** confirm the internal repo (consumes OSS as the `oss/` submodule) and hypercolor-hass/Python SDK have no logical-devices dependency and nothing beyond §3.3's migration for profiles. Python client regenerates post-78; breaking changes named in release notes per doctrine.

**Interaction with Spec 76 sequencing (as of lock, 2026-08-17):** every 78 prerequisite has landed — 2.0, 2.1, 2.3b, C1b (#195), the full 3.2 arc (#193/#196/#203), config authority (#184/#190), and 2.4 (#200). 78.0 and 78.1 are dispatchable immediately. Wave 3.1 status per §7.3; the remaining `drivers+system` batch may run on either side of 78's waves. **Wave 3.3 executes after 78.5** — the OpenAPI catalog is born against Appendix A's 81 paths, never the pre-78 surface, using the §7.2 implementation. Spec 76's remaining phases interleave by surface: Phase 5 (types restructure) follows the 78 route waves, since both churn `types::api` and the restructure should move the post-78 shapes once; wave 4.4 (store batches) coordinates with 78.4 by store — the profile import owns `profiles.json`'s retirement, 4.4 owns the rest; Phases 6/7 (engine, benchmark-gated) are surface-disjoint and may run in parallel with any 78 wave.

**Estimated effect:** 111 → 80 paths (Appendix A: 115 operations); net deletion 8 to 12k LOC across daemon + clients (dead handlers and types, the 14-route scene nesting, profiles domain, MCP second implementations, hand mirrors 3.1 no longer grows contracts for, fake-layer client code, WASM filtering, N+1 hydration).

---

## 9. Decisions resolved at draft

1. **`/scene` singleton over `/scenes/active`.** The singular resource reads as what it is (the live tree), never 404s (§1.1), and gives the zone sub-tree a stable home. The one-letter distance from `/scenes` is accepted: the two are different types (document vs collection) and confusion produces a 404 or 405, not silent misbehavior.
2. **No zone-scoped controls route.** The fake-layer hack is evidence of missing layer *identity*, not a missing route. `/scene` embeds real layer ids with a defined lifecycle (§1.4); conditional-validity shorthand routes are rejected as a class.
3. **Fine-grained mutation is live-tree-only** (§1.5), across REST and WS. Offline editing is whole-document PUT. If a real offline-structural-editing need appears, the answer is an editing-session design, not resurrecting the doubly-parameterized tree.
4. **Brightness is not captured by snapshot, but scenes may carry it** (§3.2). `activation_brightness` preserves migrated profile behavior and is explicitly opt-in for new scenes; default semantics stay clean (output owns brightness).
5. **Profiles die rather than become a scene kind.** With `layout_id` + `activation_brightness` + Display-role zones, the mapping is lossless; a `SceneKind::Snapshot` would be the same fossil with a newer name.
6. **One wire concurrency token** (§1.6). Single-writer reality plus commit-sequencer generations make per-subresource tokens theater; a false 412 costs one `/scene` re-read.
7. **`hypercolor-openapi` consumers regenerate; no REST compatibility window.** Doctrine §0. The Python SDK and HASS integration track the release that ships each wave.
8. **The effect-to-layout link is deleted** (rev 7, owner decision post-lock). An effect linking a spatial layout inverts ownership — content silently re-mapping the rig on apply — and it is the same concept `Scene.layout_id` (§3.2) now owns in the right home. The UI never adopted the link; its consumers were the CLI and the apply path itself. Deleting it also removes apply's only post-commit side-effect outcome besides power, simplifying §2.3. A client wanting effect-plus-layout in one gesture edits the scene.

## 10. Review history

- **Rev 7 (2026-08-17, owner amendment):** the effect-to-layout link is deleted (§9 decision 8, raised by the owner reviewing the promoted link types): the three `/effects/{id}/layout` routes, the `effect-layouts.json` store, apply's step-4 resolution and its `{ layout_id, applied }` outcome, and the playlist exemption that existed only because of it. Appendix A drops to 80 paths / 115 operations. `Scene.layout_id` is the sole effect-adjacent layout association.

- **Rev 6 (2026-08-17, owner reconciliation + lock):** rev 5 converged one day before ten PRs landed (#195-#204: C1b, waves 3.2b/3.2c completing the 3.2 arc, 3.1a/3.1b, 2.4, and four CI/build fixes), so every §7 amendment was re-verified against main before lock. §7.1 rewritten from "amends 3.2 acceptance criteria" to a landed/owned split — the zone-vocabulary renames and RPC tag deletion are struck as shipped; hello singletons, `zone_layout_preview` keying, backpressure classes, generated vocabulary/manifest, error vocabulary, and the remaining wire fixes transfer to 78.3/78.4 with fresh file:line receipts. §7.3 updated: C1b landed; the 3.1 `devices+scenes` batch landed ahead of its rev-5 gate (churn accepted and named); `effects+library` rescoped mid-flight to Appendix A survivors. 78.0d drops the deleted `/preview` page. Wave 3.3 explicitly sequenced after 78.5. No reviewed contract changed; this is sequencing and status reconciliation only. A CodeRabbit round on the PR then closed seven findings at lock: the §1.6 guarded set gains `PATCH /scene` and zone `PUT .../layout`; §2.3 defines the post-commit 200-with-outcomes contract and targeted repair; §3.1 defines the `PUT /scenes/{id}` whole-document request; §6.1's `transition_ms` contradiction resolves into the shared `transition` enum; Appendix A's scope names the `/ws` row's path-only matching; plus two prose/lint fixes.
- **Rev 5 (2026-08-16, coordination update):** wave C1b was discovered in flight (`nova/s76-c1b-naming-flip`, three commits plus active edits) after rev 4 converged. §7.3 reframed from *strike* to *consume*: C1b lands, its vocabulary renames become 78's floor, 78.3 deletes the transitional routes. Sequencing-only change; no reviewed contract altered.
- **Rev 4 (2026-08-16):** round-3 convergence check verified 7/8 rev-3 fixes RESOLVED with one residual (the suffix allocator could re-suffix the import's own destination on crash-replay); fixed by excluding the destination id from collision checks. Round-4 micro-check: **VERDICT: PASS. Converged.**
- **Rev 3 (2026-08-16):** round-2 codex verification pass: 15/20 round-1 findings ADDRESSED, 5 PARTIAL, 8 findings total (6 MAJOR, 2 MINOR), all adopted: every whole-layer PUT mints a fresh id (same effect included); apply sugars named in the guarded structural list; `clear_bindings` on `PatchControlsRequest` so persisted bindings are removable (nothing API-locked); idempotent import via UUIDv5-derived scene ids + numbered suffix allocation; `description` mapped and the brightness u8→f32 formula fixed; activation ordering with reported layout/brightness outcomes; profile WS removal pinned to 78.4 alone; Appendix A scoped to `/api/v1` JSON routes + `/health` with document routes excluded. Verdict trajectory: 20 → 8.
- **Rev 2 (2026-08-16):** four lens-locked codex passes over rev 1 (`codex exec`, high effort, read-only; lenses: resource-model keystones, contract semantics, Spec 76 sequencing, consumers + migration). 20 findings — 2 BLOCKER, 14 MAJOR, 4 MINOR — all verified against the tree and all adopted: display-face exception + layer-identity lifecycle (§1.3/§1.4); members addressing (§1.2); apply side-effect ordering + layout-link outcome retained + cut-only transitions + library-preset-apply deletion + playlist semantics (§2.3); scene `layout_id`/`activation_brightness` for lossless profile fold (§3.2); import-not-path-migration (§3.3); single revision token + bound-key 409 replacing the "commutative" claim (§1.6); C1b remainder reassigned + 3.1 gating + canonical-error rule + two-stage inventory strategy (§7.3/§8); WS hello/event vocabulary migration + `zone_layout_preview` scene_id removal scheduled (§7.1); MCP resource/prompt/config/docs consumers added to 78.4; hand-written docs checklists added to 78.3/78.5; Appendix A added with exact counts. Verdict trajectory: 4× NEEDS_CHANGES → rev 2.
- **Rev 1 (2026-08-16):** initial draft from the API surface review.

---

## Appendix A — Normative route inventory (80 paths, 115 operations)

Scope: the `/api/v1` surface (JSON routes plus the one `/ws` upgrade endpoint, which the convergence test matches by path without asserting a JSON shape) and `/health`. Document routes are deliberately outside the inventory and the convergence test: `/` (SPA), `/api/v1/docs`, `/api/v1/openapi.json`, and the `/mcp` mount are served pages and protocol endpoints, not API resources (`/preview` was on this list until wave 3.2c deleted the page). Config rows landed via Spec 76 wave 4.3; logical-devices rows are intentionally absent pending the §8 downstream check (re-add via spec amendment if the check fails). `⚡` marks routes whose handler is new or substantially rewritten by this spec.

| Path | Methods | Notes |
|---|---|---|
| `/health` | GET | bare probe |
| `/api/v1/system` | GET | ⚡ merges `/server` + `/status` |
| `/api/v1/system/sensors` | GET | |
| `/api/v1/system/audio-devices` | GET | ⚡ from `/audio/devices` |
| `/api/v1/output` | GET, PATCH | ⚡ power + brightness |
| `/api/v1/config` | GET | 4.3 |
| `/api/v1/config/schema` | GET | 4.3 |
| `/api/v1/config/keys/{key}` | GET, PUT, DELETE | 4.3 |
| `/api/v1/config/reset` | POST | 4.3 |
| `/api/v1/diagnose` | POST | absorbs memory + queue checks |
| `/api/v1/drivers` | GET | |
| `/api/v1/drivers/{id}/config` | GET | |
| `/api/v1/drivers/{id}/controls` | GET | |
| `/api/v1/devices` | GET | `include=attachments` |
| `/api/v1/devices/discover` | POST | |
| `/api/v1/devices/{id}` | GET, PUT, DELETE | |
| `/api/v1/devices/{id}/controls` | GET | |
| `/api/v1/devices/{id}/identify` | POST | |
| `/api/v1/devices/{id}/segments/{segment}/identify` | POST | ⚡ renamed |
| `/api/v1/devices/{id}/pair` | POST, DELETE | |
| `/api/v1/devices/{id}/attachments` | GET, PUT, DELETE | PUT takes `validate_only` |
| `/api/v1/devices/{id}/attachments/{slot}/identify` | POST | |
| `/api/v1/attachments/templates` | GET, POST | item ops deleted |
| `/api/v1/control-surfaces` | GET | |
| `/api/v1/control-surfaces/{id}` | GET | |
| `/api/v1/control-surfaces/{id}/values` | PATCH | `PatchControlsRequest` |
| `/api/v1/control-surfaces/{id}/actions/{action}` | POST | |
| `/api/v1/displays` | GET | |
| `/api/v1/displays/{id}/frame` | GET | ⚡ from `preview.jpg` |
| `/api/v1/displays/{id}/face` | GET, PUT, DELETE | |
| `/api/v1/displays/{id}/face/controls` | PATCH | `PatchControlsRequest` |
| `/api/v1/displays/{id}/face/composition` | PATCH | |
| `/api/v1/simulators/displays` | GET, POST | |
| `/api/v1/simulators/displays/{id}` | GET, PATCH, DELETE | |
| `/api/v1/simulators/displays/{id}/frame` | GET | |
| `/api/v1/effects` | GET | ⚡ filters + include |
| `/api/v1/effects/{id}` | GET | |
| `/api/v1/effects/{id}/cover` | GET | |
| `/api/v1/effects/{id}/presets` | GET | |
| `/api/v1/effects/{id}/presets/{preset}/apply` | POST | ⚡ sugar |
| `/api/v1/effects/{id}/apply` | POST | ⚡ sugar, §2.3 |
| `/api/v1/effects/install` | POST | |
| `/api/v1/effects/rescan` | POST | |
| `/api/v1/scene` | GET, PATCH | ⚡ |
| `/api/v1/scene/deactivate` | POST | ⚡ |
| `/api/v1/scene/clear` | POST | ⚡ |
| `/api/v1/scene/zones` | POST | ⚡ |
| `/api/v1/scene/zones/{zone}` | GET, PATCH, DELETE | ⚡ |
| `/api/v1/scene/zones/{zone}/layout` | PUT | ⚡ |
| `/api/v1/scene/zones/{zone}/members` | POST | ⚡ |
| `/api/v1/scene/zones/{zone}/members/{member}` | DELETE | ⚡ |
| `/api/v1/scene/zones/{zone}/layers` | GET, POST | ⚡ |
| `/api/v1/scene/zones/{zone}/layers/order` | PATCH | ⚡ |
| `/api/v1/scene/zones/{zone}/layers/{layer}` | PUT, DELETE | ⚡ |
| `/api/v1/scene/zones/{zone}/layers/{layer}/controls` | PATCH | ⚡ `PatchControlsRequest` |
| `/api/v1/scenes` | GET, POST | |
| `/api/v1/scenes/snapshot` | POST | ⚡ |
| `/api/v1/scenes/{id}` | GET, PUT, DELETE | |
| `/api/v1/scenes/{id}/activate` | POST | |
| `/api/v1/layouts` | GET, POST | |
| `/api/v1/layouts/active` | GET | |
| `/api/v1/layouts/active/preview` | PUT | |
| `/api/v1/layouts/{id}` | GET, PUT, DELETE | |
| `/api/v1/layouts/{id}/apply` | POST | |
| `/api/v1/library/presets` | GET, POST | apply route deleted, §2.3 |
| `/api/v1/library/presets/{id}` | GET, PUT, DELETE | |
| `/api/v1/library/favorites` | GET, POST | |
| `/api/v1/library/favorites/{effect}` | DELETE | |
| `/api/v1/library/playlists` | GET, POST | |
| `/api/v1/library/playlists/active` | GET | |
| `/api/v1/library/playlists/deactivate` | POST | ⚡ from `/stop` |
| `/api/v1/library/playlists/{id}` | GET, PUT, DELETE | |
| `/api/v1/library/playlists/{id}/activate` | POST | |
| `/api/v1/assets` | GET, POST | |
| `/api/v1/assets/{id}` | GET, PUT, DELETE | |
| `/api/v1/assets/{id}/blob` | GET | |
| `/api/v1/assets/{id}/thumbnail` | GET | |
| `/api/v1/capture/monitors` | GET | |
| `/api/v1/capture/source` | PUT | ⚡ from `/pick` |
| `/api/v1/ws` | GET | |
