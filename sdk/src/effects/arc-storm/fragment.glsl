#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform int iPalette;
uniform float iSpeed;
uniform float iIntensity;
uniform float iBranches;
uniform float iFlicker;

// ── Hash functions ────────────────────────────────────────────────

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// ── Value noise ───────────────────────────────────────────────────

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

// ── Ridged noise — sharp V-shaped creases for lightning ───────────

float ridge(float n) {
    n = abs(n * 2.0 - 1.0);
    return 1.0 - n * n;
}

// ── 2D rotation matrix ───────────────────────────────────────────

mat2 rot2d(float a) {
    float s = sin(a);
    float c = cos(a);
    return mat2(c, -s, s, c);
}

// ── Standard FBM for domain warping ──────────────────────────────

float fbm(vec2 p) {
    float sum = 0.0;
    float amp = 0.5;
    for (int i = 0; i < 4; i++) {
        sum += amp * vnoise(p);
        p = rot2d(0.45) * p * 2.03 + vec2(1.7, -2.1);
        amp *= 0.5;
    }
    return sum;
}

// ── Ridged FBM — the core lightning algorithm ─────────────────────
// Stacks ridged noise octaves with rotation between layers,
// creating branching tendril structures that read as electricity.

float ridgedFBM(vec2 p, int octaves) {
    float sum = 0.0;
    float amp = 0.65;
    float prev = 1.0;

    for (int i = 0; i < 5; i++) {
        if (i >= octaves) break;
        float n = ridge(vnoise(p));
        // Weight by previous octave — creates tendril branching
        sum += amp * n * prev;
        prev = n;
        p = rot2d(0.52) * p * 2.0 + vec2(5.2, 1.3);
        amp *= 0.50;
    }
    return sum;
}

// ── Palette definitions ──────────────────────────────────────────
// 2-color primary + white-hot core blending

vec3 palettePrimary(int pal) {
    if (pal == 0) return vec3(0.10, 0.92, 1.00);  // Electric — cyan
    if (pal == 1) return vec3(0.69, 0.13, 1.00);  // Violet Storm — purple
    if (pal == 2) return vec3(1.00, 0.13, 0.13);  // Crimson Arc — red
    if (pal == 3) return vec3(0.13, 1.00, 0.38);  // Toxic — green
    if (pal == 4) return vec3(0.38, 0.80, 1.00);  // Frozen — ice blue
    return vec3(0.67, 0.53, 1.00);                // Phantom — pale purple
}

vec3 paletteSecondary(int pal) {
    if (pal == 0) return vec3(0.13, 0.27, 1.00);  // Electric — blue
    if (pal == 1) return vec3(1.00, 0.13, 0.67);  // Violet Storm — magenta
    if (pal == 2) return vec3(1.00, 0.40, 0.00);  // Crimson Arc — orange
    if (pal == 3) return vec3(0.50, 1.00, 0.13);  // Toxic — yellow-green
    if (pal == 4) return vec3(0.69, 0.91, 1.00);  // Frozen — white-blue
    return vec3(0.27, 0.13, 0.67);                // Phantom — deep indigo
}

// Mix primary/secondary with white-hot core based on energy level
vec3 arcColor(float energy, int pal) {
    vec3 primary = palettePrimary(pal);
    vec3 secondary = paletteSecondary(pal);
    vec3 base = mix(secondary, primary, smoothstep(0.0, 0.6, energy));
    // White-hot core where energy peaks
    vec3 hot = mix(base, vec3(1.0, 0.97, 0.94), smoothstep(0.7, 1.0, energy));
    return hot;
}

// ── Electric field channel ────────────────────────────────────────
// Each channel is an independent lightning system with its own
// domain warping, scale, and temporal offset.

float electricChannel(vec2 p, float time, float warpAmt, int octaves, vec2 drift) {
    // Domain warp — organic drift from FBM displacement
    vec2 warpA = vec2(
        fbm(p * 0.8 + vec2(time * 0.31, -time * 0.19) + drift),
        fbm(p * 0.8 + vec2(-time * 0.23, time * 0.37) + drift + vec2(5.3, 2.7))
    );
    vec2 warped = p + (warpA - 0.5) * warpAmt;

    // Ridged FBM creates the tendril structure
    float field = ridgedFBM(warped, octaves);

    return field;
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    vec2 p = uv * 2.0 - 1.0;
    p.x *= iResolution.x / iResolution.y;

    // ── Uniform normalization ────────────────────────────────────
    float speed = max(iSpeed, 0.1);
    float time = iTime * (0.25 + speed * 0.55);
    float intensity = clamp(iIntensity * 0.01, 0.0, 1.0);
    float branches = clamp(iBranches * 0.01, 0.0, 1.0);
    float flicker = clamp(iFlicker * 0.01, 0.0, 1.0);

    // Octave count from branches control (3-5 octaves)
    int octaves = 3 + int(branches * 2.0);

    // Domain warp strength — more branches = more organic drift
    float warpStrength = 0.6 + branches * 0.8;

    // ── Discrete temporal flicker ────────────────────────────────
    // Quantized time creates electrical instability — discrete jumps
    // instead of smooth interpolation, like real discharge events.
    float flickerTime = floor(time * 12.0) / 12.0;
    float flickerNoise = hash21(vec2(flickerTime * 7.3, flickerTime * 3.1));
    float flickerMod = 1.0 - flicker * 0.6 * step(0.55, flickerNoise);

    // Occasional bright flash — heightened discharge event
    float flashNoise = hash21(vec2(floor(time * 3.0) * 11.7, 42.0));
    float flash = step(0.88, flashNoise) * flicker * 0.5;

    // ── Channel 1 — Primary tendrils (large scale) ───────────────
    float field1 = electricChannel(
        p * 1.4,
        time,
        warpStrength,
        octaves,
        vec2(0.0)
    );

    // ── Channel 2 — Secondary web (medium scale, offset) ─────────
    float field2 = electricChannel(
        rot2d(0.78) * p * 2.2 + vec2(3.1, -1.7),
        time * 1.3 + 17.0,
        warpStrength * 0.7,
        max(octaves - 1, 3),
        vec2(8.4, 3.2)
    );

    // ── Channel 3 — Fine crackle (small scale, fast) ─────────────
    float field3 = electricChannel(
        rot2d(-0.42) * p * 3.6 + vec2(-2.4, 4.8),
        time * 1.7 + 31.0,
        warpStrength * 0.5,
        max(octaves - 1, 3),
        vec2(-4.1, 7.6)
    );

    // ── Inverse-distance glow from ridged fields ─────────────────
    // k / abs(threshold - field) creates crisp bright lines exactly
    // where the ridged FBM peaks, falling off sharply to darkness.
    float glowWidth = 0.008 + intensity * 0.024;
    float glowK = 0.015 + intensity * 0.045;

    float glow1 = glowK / (abs(0.72 - field1) + glowWidth);
    float glow2 = glowK * 0.6 / (abs(0.68 - field2) + glowWidth * 1.2);
    float glow3 = glowK * 0.35 / (abs(0.65 - field3) + glowWidth * 1.5);

    // Apply flicker modulation
    glow1 *= flickerMod;
    glow2 *= mix(1.0, flickerMod, 0.7);
    glow3 *= mix(1.0, flickerMod, 0.5);

    // ── Color composition ────────────────────────────────────────
    // Each channel gets its own color mapping, then additive blend
    vec3 col1 = arcColor(clamp(glow1 * 1.2, 0.0, 1.0), iPalette) * glow1;
    vec3 col2 = arcColor(clamp(glow2 * 1.5, 0.0, 1.0), iPalette) * glow2;
    vec3 col3 = arcColor(clamp(glow3 * 2.0, 0.0, 1.0), iPalette) * glow3;

    // Additive layering — hot-white convergence where channels cross
    vec3 col = col1 + col2 + col3;

    // Flash event — brief global intensity spike
    col *= 1.0 + flash;

    // ── Very faint ambient so the background isn't pure black ────
    vec3 ambient = palettePrimary(iPalette) * 0.006;
    col += ambient;

    // ── Soft-clip tone mapping ───────────────────────────────────
    // Reinhard-style with a brightness boost to keep cores hot
    col = col / (1.0 + col * 0.4);
    col *= 1.3 + intensity * 0.4;

    // Final clamp — the tone mapping handles blowout prevention
    col = clamp(col, 0.0, 1.0);

    // Gamma — slight lift for LED gamma response
    col = pow(col, vec3(0.92));

    fragColor = vec4(col, 1.0);
}
