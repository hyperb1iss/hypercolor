/**
 * Hypercolor GLSL Math Utilities
 *
 * Reference implementations of common math helpers used across effect
 * fragment shaders. The current esbuild config loads `.glsl` files as
 * raw text (`loader: { '.glsl': 'text' }`) so `#include` directives are
 * NOT honored — shaders that want these helpers must still paste them
 * inline. Keeping a canonical copy here lets future bundler work wire
 * up real includes, and lets reviewers cite the same source.
 */

// ── Interpolation & Range ───────────────────────────────────────────────

// saturate() is not a built-in in GLSL ES 3.00, emulate with clamp
float saturate(float x) { return clamp(x, 0.0, 1.0); }
vec2 saturate(vec2 v) { return clamp(v, 0.0, 1.0); }
vec3 saturate(vec3 v) { return clamp(v, 0.0, 1.0); }
vec4 saturate(vec4 v) { return clamp(v, 0.0, 1.0); }

float remap(float value, float inMin, float inMax, float outMin, float outMax) {
    float t = (value - inMin) / max(inMax - inMin, 1.0e-6);
    return mix(outMin, outMax, clamp(t, 0.0, 1.0));
}

// ── Exponential smoothing ───────────────────────────────────────────────
//
// Frame-rate-independent lowpass — same formula used by the TypeScript
// `smoothApproach` helper in `@hypercolor/sdk/math`. `lambda` is the
// inverse time constant (higher → faster convergence).
float smoothApproach(float current, float target, float lambda, float dt) {
    if (lambda <= 0.0) return target;
    float factor = 1.0 - exp(-lambda * max(dt, 0.0));
    return current + (target - current) * factor;
}

// ── Angle & Rotation ────────────────────────────────────────────────────

mat2 rot2(float angle) {
    float c = cos(angle);
    float s = sin(angle);
    return mat2(c, -s, s, c);
}
