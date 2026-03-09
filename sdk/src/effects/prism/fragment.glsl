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

// Convenience: 3-octave FBM for domain warping
float fbm3(vec2 p) {
    return fbm(p, 3);
}

// ── Palette ───────────────────────────────────────────────────────

vec3 triGradient(float t, vec3 a, vec3 b, vec3 c) {
    t = fract(t);
    if (t < 0.5) return mix(a, b, t * 2.0);
    return mix(b, c, (t - 0.5) * 2.0);
}

vec3 paletteColor(float t, int id) {
    // 0: Crystal — white highlights, cyan, deep blue (gemstone refraction)
    if (id == 0) return triGradient(t,
        vec3(0.85, 0.95, 1.00),   // white-ice highlight
        vec3(0.00, 0.90, 1.00),   // cyan #00e5ff
        vec3(0.00, 0.10, 0.40));  // deep blue #001a66

    // 1: SilkCircuit — purple, cyan, coral
    if (id == 1) return triGradient(t,
        vec3(0.88, 0.21, 1.00),   // electric purple #e135ff
        vec3(0.50, 1.00, 0.92),   // neon cyan #80ffea
        vec3(1.00, 0.42, 0.76));  // coral #ff6ac1

    // 2: Midnight — deep indigo, purple, blue
    if (id == 2) return triGradient(t,
        vec3(0.10, 0.00, 0.40),   // deep indigo #1a0066
        vec3(0.48, 0.12, 0.64),   // purple #7b1fa2
        vec3(0.29, 0.08, 0.55));  // blue #4a148c

    // 3: Ember — dark red, orange, amber
    if (id == 3) return triGradient(t,
        vec3(0.55, 0.00, 0.00),   // dark red #8b0000
        vec3(1.00, 0.40, 0.00),   // orange #ff6600
        vec3(1.00, 0.65, 0.00));  // amber #ffa500

    // 4: Frozen — white-blue, cyan, deep navy
    if (id == 4) return triGradient(t,
        vec3(0.69, 0.91, 1.00),   // white-blue #b0e8ff
        vec3(0.00, 0.80, 1.00),   // cyan #00ccff
        vec3(0.00, 0.04, 0.16));  // deep navy #000a2a

    // 5: Neon — hot pink, electric green, cyan
    return triGradient(t,
        vec3(1.00, 0.00, 0.67),   // hot pink #ff00aa
        vec3(0.00, 1.00, 0.40),   // electric green #00ff66
        vec3(0.00, 1.00, 1.00));  // cyan #00ffff
}

// ── Main ──────────────────────────────────────────────────────────

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    vec2 centered = (uv - 0.5) * vec2(iResolution.x / iResolution.y, 1.0);

    // ── Uniform normalization ────────────────────────────────────
    float speed = max(iSpeed, 0.2);
    float time = iTime * (0.15 + speed * 0.12);
    float segments = floor(clamp(iSegments, 3.0, 12.0));
    int octaves = 3 + int(clamp(iComplexity * 0.01, 0.0, 1.0) * 4.0);  // 3-7
    float zoom = 1.5 + clamp(iZoom * 0.01, 0.0, 1.0) * 4.5;           // 1.5-6.0

    // ── Step 1: Polar coordinates ────────────────────────────────
    vec2 p = centered;
    float r = length(p);
    float a = atan(p.y, p.x);

    // Slow global rotation — the whole kaleidoscope turns
    a += time * 0.08;

    // ── Step 2: Kaleidoscope fold ────────────────────────────────
    float segAngle = TAU / segments;
    a = mod(a, segAngle);           // repeat into first segment
    a = min(a, segAngle - a);       // mirror within segment

    // ── Step 3: Cartesian from folded polar ──────────────────────
    vec2 folded = vec2(cos(a), sin(a)) * r;

    // ── Step 4: Scale by zoom ────────────────────────────────────
    vec2 q = folded * zoom;

    // ── Step 5: Domain warp for organic motion ───────────────────
    // Two independent FBM offsets create fluid, non-repetitive drift
    vec2 warpOffset1 = q + vec2(time * 0.15, -time * 0.11);
    vec2 warpOffset2 = q + vec2(5.2, 1.3) + vec2(-time * 0.12, time * 0.09);

    vec2 warped = q + vec2(
        fbm3(warpOffset1),
        fbm3(warpOffset2)
    ) * 0.4;

    // ── Step 6: Multi-scale pattern ──────────────────────────────
    // Coarse noise for broad structure, fine noise for crystalline texture
    float coarse = fbm(warped, octaves);
    float fine = fbm(warped * 2.5 + vec2(3.1, 7.7), octaves);

    // A third layer adds slow macro movement
    float macro = fbm(warped * 0.6 + vec2(-time * 0.06, time * 0.04), 3);

    // Combine — coarse dominates, fine adds sparkle, macro adds drift
    float pattern = coarse * 0.60 + fine * 0.25 + macro * 0.15;

    // ── Step 7: Contour glow (SDF neon lines) ────────────────────
    // Creates luminous contour bands at pattern thresholds —
    // the signature "refracting light through crystal" look
    float contour1 = abs(fract(pattern * 4.0) - 0.5);
    float contour2 = abs(fract(pattern * 8.0 + 0.25) - 0.5);

    float glow1 = 0.008 / (contour1 + 0.012);
    float glow2 = 0.004 / (contour2 + 0.015);

    float glow = glow1 * 0.70 + glow2 * 0.30;

    // ── Step 8: Segment boundary lines ───────────────────────────
    // Thin bright lines at the kaleidoscope fold edges
    float rawAngle = atan(centered.y, centered.x) + time * 0.08;
    float nearEdge = abs(mod(rawAngle, segAngle) - segAngle * 0.5);
    float edgeDist = nearEdge / segAngle;
    float edgeLine = 0.003 / (edgeDist + 0.005) * smoothstep(0.0, 0.15, r);
    // Fade the edge lines out with distance — subtle geometric accent
    edgeLine *= exp(-r * 2.0) * 0.15;

    // ── Step 9: Radial brightness ────────────────────────────────
    // Brighter near center, organic fade at edges
    float centerGlow = exp(-r * 1.8);
    float radialMask = 0.25 + 0.75 * centerGlow;

    // ── Step 10: Color composition ───────────────────────────────
    // Primary color from pattern position, shifted by time
    float colorT = pattern + time * 0.04;
    vec3 baseColor = paletteColor(colorT, iPalette);

    // Secondary shifted color for glow highlights
    vec3 glowColor = paletteColor(colorT + 0.33, iPalette);

    // Edge line color — bright accent
    vec3 edgeAccent = paletteColor(colorT + 0.66, iPalette);

    // Base fill — dimmer organic noise fill under the glow lines
    float fill = smoothstep(0.2, 0.8, pattern);
    vec3 fillLayer = baseColor * fill * 0.30;

    // Glow contour lines — the main visual
    vec3 glowLayer = glowColor * glow * 0.55;

    // Combine
    vec3 color = fillLayer + glowLayer;

    // Segment edge accent
    color += edgeAccent * edgeLine;

    // Apply radial mask
    color *= radialMask;

    // Core brightening — center of the kaleidoscope radiates
    float corePulse = exp(-r * 8.0) * (0.3 + 0.2 * sin(time * 1.8));
    vec3 coreColor = paletteColor(time * 0.06, iPalette);
    color += coreColor * corePulse * 0.4;

    // ── Step 11: Vignette ────────────────────────────────────────
    float vignette = smoothstep(1.40, 0.25, length(centered));
    color *= 0.75 + 0.25 * vignette;

    // ── Step 12: Tone mapping ────────────────────────────────────
    color = max(color, vec3(0.0));
    color = pow(clamp(color, 0.0, 1.0), vec3(0.96));

    fragColor = vec4(color, 1.0);
}
