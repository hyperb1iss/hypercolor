# PR #158 Deep Review: macOS Native Screen Capture and Host Input

**Branch:** `nova/macos-capture-input` (HEAD `b881c795`, rebased onto `origin/main` `8821e02b`)
**Reviewed:** 2026-08-15
**Method:** 8-lens `hyper-pr-review`, level 3 (deep). Read-only. Working-tree and untracked files included in scope.
**Scope:** 322 files, +87,354 / -2,966 vs `origin/main`, plus ~2,600 lines of uncommitted working-tree changes and 2,260 lines of untracked files (H2.3 `native/transactions.rs` + `native/lifecycle.rs`, H1.5 `macos_launcher_authority.rs`).

**Verdict: NEEDS_CHANGES.** Multiple confirmed blockers.

---

## Executive summary

The capture and input engines are well-built. The transaction claim-once model, the generation-keyed IOSurface ownership caches, the CGEventTap disable/re-enable handling, and the seqlock latency histogram all hold up under adversarial reading. The old `device_query` polling bridge is genuinely gone. The sequence-0 first-frame rejection from prior history is fixed at HEAD.

The daemon-ownership superstructure is the problem, on two axes at once. It is the direct cause of the black screen, and it is dramatically over-engineered: roughly 16,800 lines across 12 distinct concepts (owner record, session attestation, handover journal, flock guard, incarnations, launcher authority, read-probe-reread verification, owner watch, TCC canary), when the project's own spec 77 invariant 5 says the flock is supposed to be the only ownership authority. The single highest-value change in the PR is to collapse that layer: put `{owner, incarnation, session_id, credential}` inside the flock-held file itself and make handover a launchd bootout/bootstrap.

So: the compositor and the capture/input pipelines are right. The process-ownership layer is both the bug and the biggest simplification opportunity.

---

## The two live symptoms

### Black screen: an orphaned daemon

Not a UI bug. The app launched at 22:42 on Aug 14 spawned a sidecar daemon (pid 31138) that outlived the app and was still holding `:9420` at review time, reparented to launchd (ppid 1). On relaunch the supervisor spawned five fresh daemon children; each child's health probe was answered in ~1ms by the stale daemon (a fresh child cannot bind and serve that fast), then ownership verification failed with `healthy app sidecar has no matching authoritative owner publication`, the supervisor gave up after 5 restarts in 300s, and the trusted UI never received its daemon routes, so it rendered black.

Receipt: `~/Library/Application Support/hypercolor/logs/hypercolor-app.log.2026-08-15`, the 20:37 block.

### Keyboard dead while trackpad works: external secure-input holder

External to Hypercolor. The terminal app cmux (pid 1218) holds the macOS secure-input assertion (`kCGSSessionSecureInputPID`, confirmed via ioreg). With Secure Keyboard Entry asserted, a `CGEventTap` receives zero keyboard events while pointer events still flow, which is exactly the observed symptom. Restarting the terminal (or disabling its Secure Keyboard Entry) releases it.

### Immediate unblock (reversible, not yet executed)

```
kill 31138           # frees :9420; next app launch spawns a clean daemon
# then restart the terminal app to release the keyboard
```

---

## Blocking findings

### B1. Orphaned sidecar defeats owner verification (the black screen)

`crates/hypercolor-app/src/supervisor/mod.rs`

The sidecar's lifetime is not coupled to app death. The unix child is detached via `process_group(0)` (line 2423) with a no-op platform guard, so the only kill path is `ManagedDaemon::Drop`, which never runs because tray-quit calls `app.exit(0)` and `std::process::exit` skips destructors. When a stale daemon then answers `:9420`, the watchdog treats it as spawn-retry-then-give-up instead of invoking the recovery/remedy machinery that already exists (`recover_daemon_owner`, `MacosOwnerRemedy`). Every recovery path also dead-ends because AppSidecar stop authority is a retained `Child` handle the new process does not hold.

Confirmed by the logs plus live process state (pid 31138, ppid 1). The dirty `app_sidecar_recovery_needs_rearm` predicate in `ownership.rs` covers only `(AppSidecar, RequestedOwnerStarted)`, which is an in-flight owner-switch handover, not the orphaned-crash case where the journal is absent or terminal.

Fix has two parts: couple sidecar lifetime to the app (parent-death watch via kqueue `EVFILT_PROC`, or reap on `RunEvent::Exit`), and route "guard contended by stale owner" into takeover rather than the crash-loop.

### B2. Launcher authority breaks the dev loop and every non-bundle install

`crates/hypercolor-daemon/src/macos_launcher_authority.rs` (untracked, uncommitted)

Two independent startup failures, both confirmed by tracing:

- `just daemon` / `cargo run` makes the daemon's parent `cargo`, which is not in the shell allowlist (`terminal_parent_is_valid`, line 368). So `standalone` evidence is false, no owner matches, and `exact_owner()` bails "launcher authority is missing" before startup. Same under `sudo` (parent `sudo`).
- `paths_are_equal` at line 269 runs `canonicalize()?` on `<daemon_dir>/Hypercolor` before the `layout_valid` gate is consulted (line 161), so any layout without a sibling `Hypercolor` binary (standalone, homebrew, launchd) hard-errors at startup. The `.expect("validated app sidecar path has a parent")` comment shows the gate was meant to run first.

### B3. Capture transaction epoch-adoption bug (likely the live screen-capture issues)

`crates/hypercolor-macos-capture/src/native.rs` + uncommitted `native/transactions.rs`

When a source pick or interrupted-recovery restage adopts an in-flight pending request, the code remaps the stage epoch (`pending.epoch = epoch`, line 2309) but the transaction cell's generation is immutable (`transactions.rs:289`, no setter). Every generation-filtered operation (`arm_candidate_deadline` line 1782, cancel line 2854) then misses. The freshly staged candidate is insta-cancelled, `state.candidate_completion = None` clears the cell without claiming it, and the core waiter in `set_screen_capture_demand`/`reconfigure_screen_capture` either times out spuriously ~5s later via the old epoch's still-armed deadline, or hangs forever in the interleaving where prepare fails after the replacement (the completer has no Drop-cancel).

Effect: capture does not recover after a display sleep or picker change with a request in flight. The existing test passes because the fixture path bypasses `arm_candidate_deadline` and activation deliberately has no generation filter, so the fixture path works while the production path breaks. Uncommitted H2.3 work, so it is fixable before it lands.

Falsifier (non-macOS unit test): reserve a request candidate at gen 42, reserve a selection candidate at gen 43, then assert `arm_candidate_deadline(43, StreamStart, ...) == Ok(true)`. The bug predicts `Ok(false)`.

Class-kill: rekey the cell generation on adoption, or drop the generation filter in favor of a cell-identity check, plus a `Drop` on the completer that publishes `Cancelled` so no path can strand a waiter.

### B4. Two capture endpoints have no protected-control gate

`crates/hypercolor-daemon/src/api/config.rs`, `api/diagnose.rs`

Both handlers were statically confirmed to take no auth context.

- `POST /config/set` (line 82) will start screen capture, retarget the display, flip the mic on, or enable keyboard capture from any local unprivileged process with no credential and no TCC prompt: `capture.enabled`, `capture.source`, `audio.device`, `input.keyboard` all route through `apply_capture_config_transaction`. This bypasses the gated picker (`/capture/source/pick`) that guards the same `capture.source` mutation.
- `POST /diagnose {"checks":["macos_screen_parity"]}` (line 214) actuates a real screenshot-reference capture ungated. It returns aggregate delta metrics, not raw pixels, so it is not a pixel-exfiltration path, but it actuates the protected capture pipeline with no credential.

This violates the branch's own `docs/content/api/auth-and-security.md`, which states privacy-bearing capture "always require[s] an authenticated control credential, including on loopback." The loopback CSRF check blocks browsers but not native local processes.

Falsifier (each settles in seconds against a running daemon):
```
curl -s -X POST 127.0.0.1:9420/api/v1/config/set \
  -H 'content-type: application/json' -d '{"key":"capture.enabled","value":"true"}'
```
A 403 kills the finding; a 200 + live apply confirms it.

### B5. No secure-input / lock / fast-user-switch detection; held keys stick forever

`crates/hypercolor-macos-input/src/macos.rs`

The 250ms health tick (`drain_batches`, line ~709) polls only TCC, never `IsSecureEventInputEnabled`. The `SessionInterrupted` gap reason exists (`shared.rs:91`) but is dead code, constructed nowhere. The only paths that clear `pressed_keys` are a real KeyUp or a `StateGap` (`core/src/input/macos.rs:1105`).

Effect: when a terminal grabs secure input (today's scenario), `pressed_keys` stays populated indefinitely, keyboard-reactive effects stay lit, and the session still reports Live. Spec 76 §15 rule 8 requires secure input, lock, logout, and fast-user-switch to clear held state; only the TCC leg is implemented.

Fix: poll `IsSecureEventInputEnabled` next to the existing TCC poll in the health tick; on a rising edge emit `request_gap(SessionInterrupted)` (the dead variant is the designed carrier) and surface a secure-input field on `MacosInputPlatformStatus`. PID-only diagnostics keep §15 rule 4 (no app names in logs).

---

## Non-blocking findings

### N1. Scroll is broken two independent ways

- All platforms (confirmed): the JS adapter (`crates/hypercolor-core/src/effect/lightscript/frame_payload_adapter.js:177`) dropped its `/120`, making `engine.mouse.wheel` 120x larger, and the in-repo `keystrike` effect consumer (`sdk/src/effects/keystrike/main.ts:135`) was not updated, so one notch swings hue by 4.8 instead of 0.04.
- macOS only (plausible): `macos.rs:666` reads the 16.16 fixed-point `ScrollWheelEventFixedPtDeltaAxis1` with `integer_value_field`, which returns the value rounded to the nearest integer rather than the raw bits, making native scroll ~65536x too small. Those fields are documented 16.16 fixed-point and the double accessor is the one that applies scaling. On-device falsifier: run `examples/dump_macos_input.rs`, scroll one notch; `±65536` kills it, `±1` confirms. Fix is `double_value_field × Q16_16_SCALE`.

### N2. Production CPU fallback is live while specs 76 and 77 declare capture GPU-only

`crates/hypercolor-core/src/input/screen/macos.rs:1725`. Spec 77 invariant 1 ("never materializes a full frame on CPU") is a security-boundary claim that is currently false. Either land the removal (H3.5) or rewrite the invariants as target-state with the fallback named as the temporary mitigation.

### N3. Repeated tap-disable permanently kills capture with no recovery trigger

`macos.rs:538`. Two timeout-disables within the 10s window leave the tap disabled, and a disabled tap fires no callbacks, so nothing re-enables it. No `Degraded`-to-restart path exists anywhere. Sleep/wake or a debugger pause can trigger this.

### N4. CFRunLoopStop race can hang stop() forever

`macos.rs:425`. If `stop()` lands after the worker's `stopping` check but before `CFRunLoopRun` marks the loop running, `CFRunLoopStop` is a no-op, the loop runs indefinitely, and `MacosInputSession::stop` blocks forever in `join()`. Fix: install a dedicated `CFRunLoopSource` whose handler calls `CFRunLoopStop` from inside the loop.

### N5. Webview has no navigation guard while holding the session credential

`crates/hypercolor-app/src/main.rs:80` builds the window with only `on_new_window`; there is no `on_navigation`, and the capability is scoped only by window label (`capabilities/default.json`). Given an XSS sink in the bundled SPA, a remote page inherits `window.__TAURI__`, reads the 256-bit `protected_control_credential`, and drives protected capture over loopback. Add an `on_navigation` that rejects non-`tauri` origins.

### N6. macOS releases and homebrew job deleted from public CI, undisclosed in PR body

Tagged releases from `ci.yml` will ship zero macOS artifacts, and the `update-homebrew` job deletion also kills Linux formula updates as collateral. This is deliberate (public CI cannot sign), but `docs/development/RELEASING.md:44` still claims the tap gets updated and lists `HOMEBREW_TAP_TOKEN` as required, and the PR body does not mention the removal.

---

## Follow-ups

- Non-constant-time credential and API-key compare (`api/security.rs:196`, `:769`).
- Control API key grants capture from non-loopback; a privacy-bearing capability arguably should require loopback even with a control key.
- CapsLock physical release emits a phantom `Repeated` event and inflates `impossible_key_edges` each cycle (`core/src/input/macos.rs:1225`).
- `NSEvent::eventWithCGEvent` runs on a thread with no autorelease pool (`macos.rs:581`); slow leak plus console spam. Wrap the type-14 branch in `autoreleasepool`.
- `native.rs` at 9,360 lines defers its own decomposition (spec 77 H6.1); the two new submodules prove the extraction pattern.
- 1Hz re-verify loop (`supervisor/mod.rs:66`) where the repo already ships a notify-watcher (`startup/macos_owner_watch.rs`).
- Private-selector secure-input workaround (`app/src/window.rs:126`) fails silent if WebKit renames `_resetSecureInputState`; add a once-per-process warn, and note it is a Mac App Store rejection vector if MAS is ever on the table.
- The `canvas` watch channel is deliberately not control-gated; confirm no screen-source producer composites into it.

---

## Structural assessment

The ownership machinery is 12 concepts and ~16,800 diff lines (macos-owner lib 3,096 + tests 1,680, app ownership 1,303, supervisor 1,491, launcher authority 585, owner watch 780 + tests 793, canary 3,520 + tests 1,985, CLI 645, scripts 1,520). The branch defines 204 distinct public `Macos*` types.

The tell: spec 77 invariant 5 states the flock is the only ownership authority, yet the branch ships four on-disk artifacts and two locks whose mutual consistency requires the read-probe-reread verification machinery. Simpler alternative: the winner writes `{owner, incarnation, server_session_id, credential}` into the locked guard file and fsyncs; reading while probing contention becomes atomic by construction, collapsing the attestation file, the cross-file consistency code, and the drift re-read into one locked read. Handover becomes launchd bootout/bootstrap (idempotent, each launcher already self-verifies via authority evidence).

Two more structural items worth folding into the same pass: the 6,296-line TCC canary release-harness lives inside the daemon crate (a standalone `hypercolor-macos-canary` bin crate keeps the daemon at zero canary surface), and large inline test bodies plus ~35 `#[cfg(test)]` seams woven into production types in `native.rs` and `screen/macos.rs` grip internals and break on refactor.

---

## CI status

All 9 failures currently shown on the PR ran against the stale pre-rebase remote head and are already fixed on this lineage (verified: cross-target `cargo check` for linux-gnu and windows-msvc both finish clean; the E0004 non-exhaustive match and dead-code deny were fixed by the portable-compilation commit; the Intel nasm gap is fixed by an added `brew install nasm` step; the SDK Biome import-sort was re-sorted).

One new red test on the rebased branch: `every_static_router_operation_is_cataloged` (added by the branch itself, commit 859df1ad) fails because the effect-preset routes (`/effects/{id}/presets`, `/effects/{id}/presets/{preset_id}/apply`, on `origin/main` since before v0.3.2) were never added to the hand-maintained OpenAPI catalog in `api/openapi.rs`. Pre-existing gap, surfaced by the branch's stricter test. The failing test does not exist on `main` (confirmed: a `test-one` run on main matches 0 tests). Fix is two catalog entries; it will keep `Rust Test / Daemon` red until patched.

---

## What is solid (verified good, do not touch)

- Transaction core: claim-once, settlement Drop converting abandoned `Ok` to `Cancelled`, deadline rearm revisions, timeout-vs-complete races, stop quarantine released by late witness. Strong targeted tests.
- IOSurface ownership: both cache layers key on session + resource generation + surface id + shape + allocation, refresh the retained owner on hit, evict only the cache's own retention while in-flight frames hold independent Arcs; the surface lease re-verifies id/extent/format/allocation.
- Sequence-0 first-frame bug: fixed. Core shifts `frame.sequence.checked_add(1)` before the nonzero gates; epochs start at 1.
- Latency histogram: proper seqlock (odd/even generation, single writer, reader re-validates behind an acquire fence) with a mid-snapshot-write test.
- Tap disable/re-enable: timeout and user-input disables both enqueue an ordered `StateGap`, bump per-reason counters, re-enable once, degrade on repeat.
- Owner-store file I/O: TOCTOU-resistant (`symlink_metadata`, rejects symlinks/non-regular files, enforces 0600 + owner-uid, re-validates the opened fd via dev/ino); atomic writes via `O_EXCL` temp + fsync + rename + parent-dir fsync.
- Credential generation: 256-bit from `/dev/urandom`, `[REDACTED]` Debug, never serialized over HTTP (only `server_session_id` is exposed, at the exempt `/api/v1/server`).
- Launcher-authority env var is a claim only; real authority is derived from process inspection plus code-signing evidence, and env-vs-CLI disagreement is rejected. Spoofing is fail-closed; the residual is DoS-by-inherited-env.
- Loopback enforcement, WS capture-channel gating, MCP redaction, WS token log redaction, and the trusted-local bridge (in-process, not network-reachable) all check out.
- Cross-platform cfg-gating hygiene is clean at every spot checked; no performance baselines reduced.

---

## Negative space and process transparency

This was a level-3 deep pass: 8 read-only lens agents on a frozen artifact (capture core, host input, ownership/app-shell, daemon API contracts, security, CI/intent-drift, cross-platform regression, structure/sprawl), with load-bearing anchors independently grep-verified and the two live symptoms reproduced against logs and live process state. Blocking findings B1, B2, B4, and the scroll and openapi items were confirmed by code trace or static read; B3 and B5 carry named falsifiers (a non-macOS unit test and an on-device `dump_macos_input` run respectively); N1's macOS leg is plausible pending the one-notch on-device check.

No builds were run inside the lenses (read-only). The orchestrator ran `just check`, `just lint`, and `just test-crate` on the five macOS-touched crates (all green except the openapi catalog test), plus cross-target compiles via the CI lens. Not reviewed line-by-line: the 2,071-line GPU bench example, the 571-line signing C, and the excluded-from-workspace `hypercolor-ui` internals beyond the daemon-connection consumption path. The macOS Intel release fix is verified by workflow diff only, since no Intel runner run exists on this lineage.

Rebase: the branch is rebased onto `origin/main` (was 124 ahead / 3 behind, now 124/0); local `main` is rebased too, with the unpushed docs commit clean on top. Autostash preserved the entire dirty tree.

---

## Addendum: fix pass landed (2026-08-15)

Every blocking and non-blocking finding above is fixed on the branch as of
`c1d78b8e` (ten commits, `ebb1dfb4..c1d78b8e`), verified by four adversarial
Opus verification agents plus full gates (workspace clippy at deny-warnings,
crate suites, SDK typecheck/lint/tests). Three corrections the verification
round made to this review's own claims:

- N1's all-platform leg reversed: the adapter's missing `/120` is the
  contract, not the bug. Spec 76 §wheel defines `mouse.wheel` as 1/120-notch
  units, the SDK bridge test pins `-240` per two notches, and commit
  `ec5dfc54` removed the division deliberately. The real defect was the
  keystrike consumer's stale per-notch factor, fixed there. The macOS
  fixed-point leg was confirmed and fixed as written (double accessor,
  Q16.16 scale-back).
- The "all green except openapi" test claim was too strong: cargo's
  per-binary fail-fast meant the render-thread suite never ran during the
  review. It fails 3-5/46 under default intra-binary parallelism on
  high-core machines (passes 46/46 single-threaded; `origin/main` flakes
  1/46 the same way). Test-harness fragility, tracked in sibyl, not a
  product defect proven.
- The review's B1 fix sketch missed that the terminal conflict-exit branch
  is the *primary* orphan signal; the landed fix reclaims from all three
  watchdog failure paths.

Two review-adjacent surfaces were found and deliberately deferred (tracked
in sibyl): the consent model for capture-reactive effect applies, and MCP
tool authentication. The structural ownership collapse and platform-code
disentanglement remain the follow-up architecture phase.
