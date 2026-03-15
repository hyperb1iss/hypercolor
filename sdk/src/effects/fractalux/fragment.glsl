#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform int iPalette;
uniform float iSpeed;
uniform float iSegments;
uniform float iComplexity;
uniform float iZoom;
uniform float iBrightness;
uniform float iSaturation;
uniform float iWarp;
uniform float iPulse;
uniform int iMotion;

// ── Constants ─────────────────────────────────────────────────────

const float TAU = 6.28318530718;
const float PI  = 3.14159265359;

// ── Hash & noise primitives ───────────────────────────────────────

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

float vnoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    f = f * f * (3.0 - 2.0 * f);

    float a = hash21(i);
    float b = hash21(i + vec2(1.0, 0.0));
    float c = hash21(i + vec2(0.0, 1.0));
    float d = hash21(i + vec2(1.0, 1.0));

    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// ── 2D rotation ───────────────────────────────────────────────────

mat2 rot2d(float a) {
    float s = sin(a);
    float c = cos(a);
    return mat2(c, -s, s, c);
}

// ── FBM with variable octaves ─────────────────────────────────────

float fbm(vec2 p, int octaves) {
    float sum = 0.0;
    float amp = 0.5;
    for (int i = 0; i < 7; i++) {
        if (i >= octaves) break;
        sum += amp * vnoise(p);
        p = rot2d(0.42) * p * 2.04 + vec2(7.3, -4.1);
        amp *= 0.48;
    }
    return sum;
}

float fbm3(vec2 p) { return fbm(p, 3); }

// ── Palettes ──────────────────────────────────────────────────────
// Indices MUST match combo order: Aurora, Crystal, Ember, Frozen,
// Midnight, Neon, Psychedelic, SilkCircuit

vec3 triGradient(float t, vec3 a, vec3 b, vec3 c) {
    t = fract(t);
    if (t < 0.5) return mix(a, b, t * 2.0);
    return mix(b, c, (t - 0.5) * 2.0);
}

vec3 quadGradient(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    t = fract(t);
    if (t < 0.333) return mix(a, b, t * 3.0);
    if (t < 0.667) return mix(b, c, (t - 0.333) * 3.0);
    return mix(c, d, (t - 0.667) * 3.0);
}

vec3 paletteColor(float t, int id) {
    // 0: Aurora — northern lights: green → teal → violet → pink
    if (id == 0) return quadGradient(t,
        vec3(0.10, 1.00, 0.45),
        vec3(0.00, 0.80, 0.70),
        vec3(0.55, 0.10, 1.00),
        vec3(1.00, 0.25, 0.65));

    // 1: Crystal — ice refraction: white → vivid cyan → deep sapphire
    if (id == 1) return triGradient(t,
        vec3(0.92, 0.98, 1.00),
        vec3(0.00, 0.95, 1.00),
        vec3(0.05, 0.18, 0.60));

    // 2: Ember — molten fire: crimson → orange → amber → hot yellow
    if (id == 2) return quadGradient(t,
        vec3(0.72, 0.00, 0.05),
        vec3(1.00, 0.45, 0.00),
        vec3(1.00, 0.72, 0.00),
        vec3(1.00, 0.95, 0.20));

    // 3: Frozen — arctic abyss: ice → cyan → deep navy
    if (id == 3) return triGradient(t,
        vec3(0.78, 0.94, 1.00),
        vec3(0.00, 0.88, 1.00),
        vec3(0.02, 0.06, 0.25));

    // 4: Midnight — deep space: indigo → vivid purple → electric blue → violet
    if (id == 4) return quadGradient(t,
        vec3(0.12, 0.00, 0.50),
        vec3(0.60, 0.15, 0.85),
        vec3(0.25, 0.10, 0.95),
        vec3(0.40, 0.00, 0.65));

    // 5: Neon — electric: hot magenta → neon green → cyan
    if (id == 5) return triGradient(t,
        vec3(1.00, 0.00, 0.72),
        vec3(0.00, 1.00, 0.45),
        vec3(0.00, 1.00, 1.00));

    // 6: Psychedelic — full spectrum rainbow via cosine palette
    if (id == 6) return 0.5 + 0.5 * cos(TAU * (t + vec3(0.0, 0.33, 0.67)));

    // 7: SilkCircuit — electric purple → neon cyan → vivid coral
    return triGradient(t,
        vec3(0.92, 0.22, 1.00),
        vec3(0.50, 1.00, 0.92),
        vec3(1.00, 0.44, 0.78));
}

// ── Main ──────────────────────────────────────────────────────────

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    vec2 centered = (uv - 0.5) * vec2(iResolution.x / iResolution.y, 1.0);

    // ── Normalize controls ──────────────────────────────────────
    float speed    = max(iSpeed, 0.2);
    float time     = iTime * (0.15 + speed * 0.12);
    float segments = floor(clamp(iSegments, 3.0, 12.0));
    int   octaves  = 3 + int(clamp(iComplexity * 0.01, 0.0, 1.0) * 4.0);
    float zoom     = 1.5 + clamp(iZoom * 0.01, 0.0, 1.0) * 4.5;
    float bright   = 0.6 + clamp(iBrightness * 0.01, 0.0, 1.0) * 1.4;
    float sat      = 1.0 + clamp(iSaturation * 0.01, 0.0, 1.0) * 1.5;
    float warpStr  = 0.1 + clamp(iWarp * 0.01, 0.0, 1.0) * 0.9;
    float pulseStr = clamp(iPulse * 0.01, 0.0, 1.0);

    // ── Motion: Drift ───────────────────────────────────────────
    // Center wanders organically — looking through a moving crystal
    if (iMotion == 3 || iMotion == 4) {
        vec2 drift = vec2(
            sin(time * 0.23) * 0.18 + sin(time * 0.37) * 0.07,
            cos(time * 0.31) * 0.14 + cos(time * 0.19) * 0.05
        );
        centered -= drift;
    }

    // ── Motion: Breathe ─────────────────────────────────────────
    // Zoom oscillates in and out — mesmerizing depth pulse
    if (iMotion == 1 || iMotion == 4) {
        float breath = 1.0 + pulseStr * 0.5 * sin(time * 0.45);
        zoom *= breath;
    }

    // ── Pulse modulation ────────────────────────────────────────
    // Radial brightness wave emanating from center
    float pulse = 1.0 + pulseStr * 0.25 * sin(time * 0.7 + length(centered) * 3.0);

    // ── Step 1: Polar coordinates ───────────────────────────────
    vec2 p = centered;
    float r = length(p);
    float a = atan(p.y, p.x);

    // Global rotation
    a += time * 0.08;

    // ── Motion: Spiral ──────────────────────────────────────────
    // Rotation accelerates with radius — hypnotic inward vortex
    if (iMotion == 2 || iMotion == 4) {
        a += r * time * 0.35;
    }

    // ── Step 2: Kaleidoscope fold ───────────────────────────────
    float segAngle = TAU / segments;
    a = mod(a, segAngle);
    a = min(a, segAngle - a);

    // ── Step 3: Cartesian from folded polar ─────────────────────
    vec2 folded = vec2(cos(a), sin(a)) * r;

    // ── Step 4: Scale by zoom ───────────────────────────────────
    vec2 q = folded * zoom;

    // ── Step 5: Domain warp ─────────────────────────────────────
    vec2 warpOffset1 = q + vec2(time * 0.15, -time * 0.11);
    vec2 warpOffset2 = q + vec2(5.2, 1.3) + vec2(-time * 0.12, time * 0.09);

    vec2 warped = q + vec2(
        fbm3(warpOffset1),
        fbm3(warpOffset2)
    ) * warpStr;

    // ── Step 6: Multi-scale pattern ─────────────────────────────
    float coarse = fbm(warped, octaves);
    float fine   = fbm(warped * 2.5 + vec2(3.1, 7.7), octaves);
    float macro  = fbm(warped * 0.6 + vec2(-time * 0.06, time * 0.04), 3);
    float pattern = coarse * 0.55 + fine * 0.28 + macro * 0.17;

    // ── Step 7: Contour glow (3-tier neon refraction lines) ─────
    float contour1 = abs(fract(pattern * 4.0) - 0.5);
    float contour2 = abs(fract(pattern * 8.0 + 0.25) - 0.5);
    float contour3 = abs(fract(pattern * 16.0 + 0.5) - 0.5);

    float glow1 = 0.012 / (contour1 + 0.010);
    float glow2 = 0.006 / (contour2 + 0.010);
    float glow3 = 0.003 / (contour3 + 0.012);

    float glow = glow1 * 0.55 + glow2 * 0.30 + glow3 * 0.15;

    // ── Step 8: Segment boundary lines ──────────────────────────
    float rawAngle = atan(centered.y, centered.x) + time * 0.08;
    if (iMotion == 2 || iMotion == 4) {
        rawAngle += length(centered) * time * 0.35;
    }
    float nearEdge = abs(mod(rawAngle, segAngle) - segAngle * 0.5);
    float edgeDist = nearEdge / segAngle;
    float edgeLine = 0.004 / (edgeDist + 0.004) * smoothstep(0.0, 0.12, r);
    edgeLine *= exp(-r * 1.5) * 0.22;

    // ── Step 9: Radial brightness ───────────────────────────────
    float centerGlow = exp(-r * 1.4);
    float radialMask = 0.40 + 0.60 * centerGlow;

    // ── Step 10: Chromatic dispersion ───────────────────────────
    // Per-channel palette offset for prismatic rainbow fringing
    float dispersion = warpStr * 0.035;
    float colorTR = pattern + time * 0.04 + dispersion;
    float colorTG = pattern + time * 0.04;
    float colorTB = pattern + time * 0.04 - dispersion;

    vec3 baseColor = vec3(
        paletteColor(colorTR, iPalette).r,
        paletteColor(colorTG, iPalette).g,
        paletteColor(colorTB, iPalette).b
    );

    vec3 glowColor = vec3(
        paletteColor(colorTR + 0.33, iPalette).r,
        paletteColor(colorTG + 0.33, iPalette).g,
        paletteColor(colorTB + 0.33, iPalette).b
    );

    // Edge accent — rainbow-shifted along the boundary
    float edgeHue = rawAngle / TAU;
    vec3 edgeAccent = paletteColor(edgeHue + time * 0.08, iPalette);

    // ── Step 11: Color composition ──────────────────────────────
    float fill = smoothstep(0.08, 0.92, pattern);
    vec3 fillLayer = baseColor * fill * 0.50 * bright;
    vec3 glowLayer = glowColor * glow * 0.70 * bright;

    vec3 color = fillLayer + glowLayer;

    // Segment edge accent
    color += edgeAccent * edgeLine * bright;

    // Apply radial mask
    color *= radialMask;

    // Core radiance — the heart of the kaleidoscope burns hot
    float corePulse = exp(-r * 5.5) * (0.45 + 0.30 * sin(time * 1.8));
    vec3 coreColor = paletteColor(time * 0.06, iPalette);
    color += coreColor * corePulse * 0.55 * bright;

    // ── Step 12: Pulse modulation ───────────────────────────────
    color *= pulse;

    // ── Step 13: Saturation boost ───────────────────────────────
    float lum = dot(color, vec3(0.299, 0.587, 0.114));
    color = mix(vec3(lum), color, sat);

    // ── Step 14: Vignette (gentle) ──────────────────────────────
    float vignette = smoothstep(1.7, 0.15, length(centered));
    color *= 0.90 + 0.10 * vignette;

    // ── Step 15: Tone mapping ───────────────────────────────────
    color = max(color, vec3(0.0));
    color = color / (1.0 + color * 0.18);   // soft Reinhard preserves highlights
    color = pow(color, vec3(0.88));          // gamma lift for vibrancy

    fragColor = vec4(color, 1.0);
}
