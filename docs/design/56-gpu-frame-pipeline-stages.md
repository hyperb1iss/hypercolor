# 56 — GPU Frame Pipeline Stages 🔮

**Status:** Proposed
**Scope:** `hypercolor-daemon` render thread — `sparkleflinger/gpu*`, `frame_sampling.rs`, `pipeline_runtime.rs`
**Motivation:** Findings from the June 2026 render-pipeline deep review

## Problem

The GPU compositor's deferred-work state is spread across ~12 orthogonal
`Option` fields on `GpuSparkleFlinger`:

```
cached_composition_key      pending_output_submission
cached_readback_surface     pending_preview_readback
cached_preview_surfaces     pending_preview_submission
ready_preview_surface       pending_preview_map
cached_sample_result        output_generation
current_output              producer_texture_generation
```

Three subtly different cleanup methods (`discard_superseded_preview_work`,
`clear_superseded_preview_outputs`, `discard_ready_and_pending_preview_surface`)
each reset a different subset, and every compose path must pick the right one.
On top of this, `frame_sampling.rs` runs a ~400-line flag machine
(`gpu_sample_deferred` / `retry_hit` / `stale` / `saturated`,
`can_hold_published_frame` / `can_reuse_published_frame`,
`should_queue_followup_sampling`) to sequence deferred zone sampling against
composition.

Each piece is individually defensible. The composition is unverifiable by
inspection, and it has already produced real bugs:

- The stacked-effects blackout fixed in `1832bf64` was a params write racing a
  deferred encoder — an ordering invariant no single field owned.
- `pending_output_submission` (an **unsubmitted command encoder**) can be
  silently dropped by two of the three cleanup methods. Today this is safe only
  because zone sampling happens to chain and submit the encoder in the same
  frame. Nothing enforces it.

## Proposal

Replace the constellation with one explicit per-frame stage value owned by the
compositor:

```rust
/// Work produced by compose() that later stages consume.
/// Exactly one exists per composed frame; dropping one un-submitted is a bug.
struct FrameInFlight {
    generation: u64,
    encoder: EncoderState,        // Building(CommandEncoder) | Submitted(SubmissionIndex)
    readbacks: Vec<StagedReadback>, // preview / full-size canvas / display finalize
}

enum StagedReadback {
    Preview { request: PreviewSurfaceRequest, slot: usize, stage: ReadbackStage },
    SamplingCanvas { slot: usize, stage: ReadbackStage },
}

enum ReadbackStage { Encoded, Submitted, Mapping(Receiver<…>), Ready(PublishedSurface) }
```

Rules:

1. **One owner.** `compose()` returns/holds exactly one `FrameInFlight`.
   Consumers (zone sampling, preview resolve, display lanes, device output)
   ask it to advance; they never reach into compositor fields.
2. **No silent drops.** `FrameInFlight` panics in debug builds if dropped while
   `encoder` is `Building` and any `StagedReadback` exists. Superseding a frame
   is an explicit `supersede()` that submits-or-discards deliberately.
3. **Caches keyed by generation.** `cached_readback_surface`,
   `cached_preview_surfaces`, and `cached_sample_result` become one
   `HashMap<CacheKey, CachedOutput>` whose entries carry the generation that
   produced them; invalidation is "generation advanced", not bespoke clears.
4. **Sampling state machine collapses.** `frame_sampling.rs` keeps only
   `DeferredSamplingState` (already exists) plus the published-frame-reuse
   decision; the retry/stale/saturated juggling moves behind
   `FrameInFlight::sample_zones(...) -> SampleOutcome` with a small enum:
   `Fresh(zones) | Deferred | ReusePublished | CpuFallback(canvas)`.

## What this deletes (not relocates)

- The three cleanup methods become one `supersede()`.
- `pending_preview_readback` / `pending_preview_submission` /
  `pending_preview_map` merge into `ReadbackStage` (a value can only be in one
  stage — the type makes the illegal states unrepresentable).
- The `gpu_sample_*` boolean flags in `LedSamplingOutcome` shrink to the
  `SampleOutcome` enum plus telemetry counters.

## Migration

1. Introduce `FrameInFlight` wrapping the existing fields; mechanical, no
   behavior change. Debug-assert on drop lands here.
2. Move preview stages into `ReadbackStage`; delete the three cleanup methods.
3. Move zone-sampling dispatch behind `sample_zones`; shrink
   `frame_sampling.rs`.
4. Unify caches under generation keys.

Each step is independently shippable and `just verify`-gated. Step 1 is cheap
insurance and worth doing immediately; steps 2-4 should ride behind a quiet
period, not alongside feature work.

## Non-goals

- No changes to the CPU compositor, bus, spatial sampler, or device output —
  the review found those healthy.
- No public API changes; this is internal to the render thread.
