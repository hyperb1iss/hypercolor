# Hypercolor patches to servo-script

This directory vendors `servo-script` 0.8.0, Servo's `components/script`
crate, from crates.io. The files are the published package exactly as
`servo-script-0.8.0.crate` unpacks (checksum
`b6c3e52be4165b3f98b992f384156b049d30b769004a96cf52903ea27cd32f9b`, upstream
commit `4e136d8ef31b7c4748fbfac606ca463f3efd7537` per
`.cargo_vcs_info.json`), plus this file. The root `Cargo.toml` swaps it in
through `[patch.crates-io]`, so every other Servo crate still comes from
crates.io. The crate is excluded from the workspace and keeps upstream's
formatting and MPL-2.0 license headers.

The commit that moved this directory to a new Servo release carries the
published crate with no source edits. Each patch below lands in its own
later commit, so `git log -p -- vendor/servo-script` from that commit on
shows every change against the published crate.

One build setting differs from the published manifest: `Cargo.toml` ends
with a `[lints.rust]` table that allows warnings. Cargo caps lints for
crates.io dependencies but not for path dependencies, so without it every
workspace build would print upstream's rustc warnings. Deny-level lints
still apply.

## Patches

### 1. Release the caller's reference to a script private

Files: `scripting/script_module.rs` (`set_script_source_private`, used by
`ModuleTree::create_a_javascript_module_script`) and
`scripting/script_execution.rs` (`maybe_associate_with_script`).

Upstream hands a fresh `Rc<BaseScript>` to `SetScriptPrivate` or
`SetModulePrivate` through `Rc::into_raw` and never reclaims that
reference. Both calls end in `ScriptSourceObject::setPrivate`
(`js/src/vm/JSScript.cpp`), which releases the previous private and calls
the runtime's add-ref hook for the new one. Servo installs that hook pair
for every JS runtime in `EnsureModuleHooksInitialized`:
`host_add_ref_top_level_script` clones the `Rc` and forgets the clone, and
`host_release_top_level_script` drops one reference. `ScriptSourceObject`
finalization calls `setPrivate(UndefinedValue())`, which releases the
engine's reference. The engine's reference is therefore balanced on its
own, and the `into_raw` one leaks one `BaseScript` per compiled classic
script or module. Embedders that call `evaluate_javascript` every frame
compile one classic script per call, so the leak grows without bound. The
module path gets the same correction. Before servo-script 0.8.0 a module's
private also held a `Trusted<GlobalScope>` that kept the pair alive
regardless; upstream dropped it (servo/servo#48952), so module privates
now free with their script source too. Hypercolor's built-in effects load
classic scripts only.

The patch reclaims the caller's reference right after the engine took its
own, so the `BaseScript` lives exactly as long as its script source. If
no add-ref hook ran (the strong count is still 1), it leaks the reference
as upstream does instead of leaving the private dangling. Every reader of
the private (`module_script_from_reference_private` and its callers in
`scripting/module_loading.rs`, `scripting/script_module.rs`, and
`event_loop/timers.rs`) borrows it while the script or module is rooted or
on the stack, and none of them keeps the pointer. The one place that
stores it, `ReferringScript` (`scripting/import_map.rs`), holds a
`&BaseScript` bound to the rooted handle's lifetime.

Upstream status: servo-script 0.8.0 and Servo `main` at `c32ccc1a4edc`
(2026-11-14) still use `Rc::into_raw` in both places.

### 2. Collect after sustained embedder script evaluation

File: `event_loop/script_thread.rs` (`EvaluationGc`,
`ScriptThread::maybe_collect_after_evaluations`, and one line in
`handle_evaluate_javascript`).

Each `evaluate_javascript` call compiles a new classic script. After it
runs, most of what it leaves (the `ScriptSource` and its text, bytecode,
and for sources of 256 characters or more an entry in the runtime's pending
source compression list, which only a major GC drains outside
SpiderMonkey's own tests) is malloc memory that SpiderMonkey does not count
toward any GC trigger. What it does count is small: the `JSScript` and
`ScriptSourceObject` cells toward the zone's GC heap trigger, which starts
at 27 MB times the heap growth factor, and the script's `PrivateScriptData`
toward the malloc trigger. Gecko covers this kind of garbage with its own
idle and timer-driven GC scheduling; Servo 0.8 has none, and its one
`JS_MaybeGC` call runs on window suspension, which an actively evaluated
page never reaches. A page driven by one evaluation per frame and little
other allocation therefore runs for a long time without a major GC while
its garbage grows. SpiderMonkey's public accounting hook,
`JS::AddAssociatedMemory`, needs the owning object, and the owners here
(`ScriptSourceObject` and `JSScript`) are engine internals with no public
accessor, so this crate cannot hand the memory to the engine's own
heuristics.

The patch counts embedder evaluations and the source bytes they compile.
At the end of each `handle_messages` batch, after the rendering update, the
script thread runs `JS_GC` once that traffic has gone on for 60 seconds or
compiled 16 MiB of source without a major GC in between. Any major GC,
including one SpiderMonkey starts on its own, restarts the window, and a
thread that sees no evaluations never collects. Nothing here is reachable
from page script. A script thread whose pages receive no embedder
evaluations behaves exactly as upstream; on one that does, `JS_GC` collects
the whole runtime, including any same-origin page that shares the thread.

Upstream status: servo-script 0.8.0 has no idle, periodic, or
evaluation-driven collection. Its explicit `JS_GC` calls are
`ScriptThreadMessage::TriggerGarbageCollection` (which page script reaches
through `servointernals` when an embedder exposes it), the `TestBinding`
and `TestUtils.gc()` testing hooks, and the worklet threads' own
collections (a cold backup collects after each batch of control messages,
and a backup past its heap-size threshold collects too). Window suspension
calls `JS_MaybeGC` (servo/servo#49425), which can collect early at
SpiderMonkey's eager allocation thresholds (85 or 90 percent of the normal
trigger) but still only counts memory the engine tracks. Suspension happens
only when a document stops being fully active, on navigation for instance;
hiding or throttling a webview, which is how Hypercolor parks one, does not
suspend it. Servo `main` at `c32ccc1a4edc` (2026-11-14) has the same gap.
servo/servo#48283, still open, proposes an idle collection after page load,
which would not cover a page that keeps receiving evaluations.

A forced collection discards the page's JIT code. Outside its testing
hooks, SpiderMonkey keeps JIT code across a GC only when its compartment
has a frame on the stack, the realm asked to keep it, or the realm reported
recent animation activity. None of those holds here: the collection runs
with no script on the stack, and Servo never asks to keep JIT code or
reports animation activity. A JIT-heavy page therefore recompiles hot
functions once a minute while evaluation traffic lasts.

## Removal rule

Drop this directory and the `[patch.crates-io]` entry once a Servo release
carries equivalent fixes. Until then, bumping Servo means re-vendoring
`servo-script` at the new version and reapplying each patch above (or
confirming upstream made it unnecessary) in the same change. A patch entry
for 0.8.0 does nothing for any other `servo-script` version: Cargo only
warns that the patch went unused and builds the unpatched crate from
crates.io. `servo` pins its script crate exactly, so even a patch release
of `servo` would bypass the vendored copy. Dependabot ignores `servo-script`
and `servo` for that reason, and `scripts/check-vendored-patches.sh` fails
CI when a version bump leaves this patch unused, a second `servo-script`
enters the graph, or a config-level `[patch]` points it elsewhere. It does
not police deliberate local overrides such as Cargo's `paths` setting.

`[patch]` only applies to builds of this workspace. A `hypercolor-core`
published to crates.io would build against the unpatched registry crate and
lose every patch listed above until a Servo release carries them.
