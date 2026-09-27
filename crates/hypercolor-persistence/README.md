# hypercolor-persistence

*Durable file replacement and the process-wide flush registry every store writes
through.*

Hypercolor has many small JSON stores (scenes, layouts, favorites, credentials,
attachment profiles) that are written from different tasks and must survive a
crash or a power loss mid-write. This crate is the single mechanism they all go
through. It orders concurrent writers to the same destination inside the
process, writes through a temp file plus an atomic replacement, retries in the
background when a replacement fails, and registers every live destination so
shutdown can wait for all of them at once.

The in-process coordinators handle same-process ordering. Cross-process
ownership of these files is the daemon's single-instance guard, not this crate.

## Workspace position

**Depends on:** `hypercolor-platform-fs` (for the actual durable replacement),
`chrono` (generation timestamps), `serde`, `serde_json`, `tempfile`,
`thiserror`, `tracing`.

**Depended on by:** `hypercolor-core` (which re-exports it as
`hypercolor_core::persistence`) and `hypercolor-driver-support`.

## Key types

**Writing**

- `AtomicFileWriter` — the generation coordinator for one stable destination.
  Construct with `new(path)`, or `with_file_mode(path, mode)` when the file must
  be private; the mode is a property of the destination, so the newest explicit
  request wins for every writer sharing that path, and non-Unix platforms ignore
  it and inherit directory permissions. `write(payload)` is the simple path.
- `AtomicWriteReservation` — a write generation reserved at the owning store's
  snapshot boundary, before the payload exists. Reserving first is what keeps a
  slow serializer from committing over a newer snapshot. `admit(payload)` turns
  it into an `AdmittedAtomicWrite`.
- `AdmittedAtomicWrite` — a complete payload admitted as the newest durable
  intent for its destination. `commit()` or `commit_stage_aware()` finish it.
- `AtomicWriteOutcome`, `AtomicWriteCommitResult` — what a commit did, including
  whether a newer generation superseded it.
- `write_atomic(path, payload)` — one-shot convenience for callers with no
  generation ordering to protect.
- `serialize_json_pretty(value)` — the shared snapshot serializer.

**Flushing**

- `flush_all(timeout)` — flushes every live destination within one shared
  deadline and returns a `PersistenceFlushReport`. Runtime retry workers stay
  alive after the bounded observation returns.
- `PersistenceFlushOutcome` — per-destination result: `Clean` (nothing pending),
  `Written` (the dirty snapshot committed), or `Superseded` (a newer generation
  replaced it).
- `PersistenceFlushReport` — aggregate counts plus `errors()` and
  `is_complete()`.
- `PersistenceFlushError` — a dirty snapshot that did not converge before its
  deadline.

**History**

- `HistoryPolicy`: how many previous generations a destination keeps, in which
  directory, and how long content must stay current before it is worth keeping.
  `AtomicFileWriter::enable_history(policy)` attaches it to the destination for
  the rest of the process.
- Before a replacement, the outgoing content is copied into the history
  directory as `{id:06}-{replaced_at}{extension}`, durably, so a crash leaves
  either the old file with its copy already kept or the new file. Content
  identical to the new payload, content the newest generation already holds,
  and content this process wrote less than `min_interval` ago are skipped, so
  rotations of one destination are at least `min_interval` apart. The oldest
  generations beyond the budget are removed.
- `capture_next_writes()` waives the interval once per destination (the daemon
  calls it before its shutdown saves).
- `list_generations(directory)` and `restore_generation(path, id)` read and
  restore. A restore is an ordinary write, so the content it replaces becomes a
  new generation and the restore can be undone.
- `set_replacement_observer(callback)` is told about every replacement that
  changed a destination's bytes, on the thread that made it. The daemon uses it
  to attribute store changes to API requests.

**Errors**

- `PersistenceError` — names the stage that failed: retry-supervisor
  initialization, snapshot serialization, invalid destination, directory
  creation, and the write and replacement stages.
- `HistoryError`: a history listing or restore that could not complete.

## Feature flags

| Feature | What it gates |
|---|---|
| `persistence-test-hooks` | Failure-injection hooks (`set_injected_serialization_failures` and the writer's replace and directory-sync injectors) used by the durability test suites. Not for production builds. |

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source
RGB lighting orchestration for Linux, Windows, and macOS. Licensed under
Apache-2.0.
