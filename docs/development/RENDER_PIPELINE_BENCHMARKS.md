# Render Pipeline Benchmarks

This is the local evidence path for deciding whether compositor changes make
the pipeline faster, steadier, or just more complicated.

## Commands

```bash
just bench-smoke
just bench-daemon daemon_sparkleflinger
just bench-daemon daemon_publish_handoff
just bench-daemon daemon_render_pipeline
```

For before/after work:

```bash
just bench-daemon -- --save-baseline pre-change
just bench-daemon -- --baseline pre-change
```

Criterion reports land under `target/criterion/`.

## SparkleFlinger Scenarios

`daemon_sparkleflinger` covers the compositor decision surface:

- `single_replace_bypass` measures the zero-composition handoff path.
- `alpha_two_layer_compose` measures the normal 320x200 transition shape.
- `alpha_two_layer_compose_640x480` measures preview-resolution CPU composition.
- `alpha_two_layer_compose_640x480_fresh` defeats CPU replay caching.
- `multi_blend_alpha_add_screen_640x480` measures representative face composition with alpha, additive glow, and screen overlay layers.
- `cpu_zone_sample_640x480` and `gpu_zone_sample_640x480` isolate LED sampling.
- `cpu_compose_and_zone_sample_640x480` and `gpu_compose_and_zone_sample_640x480` measure end-to-end preview composition plus LED sampling.
- `gpu_*_no_readback` separates GPU composition dispatch cost from readback cost.
- `gpu_alpha_two_layer_compose_640x480_scaled_preview_320x240` measures GPU preview scaling plus preview readback.

## Decision Rule

Keep CPU as the default compositor unless GPU improves one of these on the same
machine and same scene:

- End-to-end p95 or p99 frame latency is meaningfully lower.
- GPU sampling removes enough CPU sampling cost to offset dispatch and readback.
- Preview scaling moves off the render thread without increasing frame age.
- No-readback GPU paths are fast enough to justify routing display-only faces through GPU.

If GPU only wins no-readback microbenches but loses end-to-end readback or LED
sampling, keep it explicit and report the fallback reason through status and
metrics.
