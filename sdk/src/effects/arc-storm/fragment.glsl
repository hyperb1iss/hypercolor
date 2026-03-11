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

struct ArcPalette {
    vec3 deep;
    vec3 secondary;
    vec3 primary;
    vec3 contrast;
    vec3 accent;
    vec3 core;
};

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
// Multi-stop discharge ramps with tinted cores per theme.

float saturate(float value) {
    return clamp(value, 0.0, 1.0);
}

ArcPalette getPalette(int pal) {
    ArcPalette palette;

    if (pal == 0) {
        palette.deep = vec3(0.03, 0.08, 0.28);       // Electric — midnight blue
        palette.secondary = vec3(0.08, 0.32, 0.96);  // Electric — blue
        palette.primary = vec3(0.10, 0.92, 1.00);    // Electric — cyan
        palette.contrast = vec3(0.72, 0.20, 1.00);   // Electric — violet split
        palette.accent = vec3(0.46, 0.70, 1.00);     // Electric — azure
        palette.core = vec3(0.78, 0.90, 1.00);       // Electric — icy blue
        return palette;
    }

    if (pal == 1) {
        palette.deep = vec3(0.10, 0.02, 0.22);       // SilkCircuit Storm — deep violet
        palette.secondary = vec3(0.48, 0.06, 0.96);  // SilkCircuit Storm — electric purple
        palette.primary = vec3(0.88, 0.14, 1.00);    // SilkCircuit Storm — neon magenta
        palette.contrast = vec3(0.12, 0.82, 1.00);   // SilkCircuit Storm — neon cyan
        palette.accent = vec3(1.00, 0.22, 0.58);     // SilkCircuit Storm — coral
        palette.core = vec3(0.35, 0.96, 1.00);       // SilkCircuit Storm — neon cyan
        return palette;
    }

    if (pal == 2) {
        palette.deep = vec3(0.22, 0.02, 0.04);       // Crimson Arc — ember wine
        palette.secondary = vec3(0.72, 0.05, 0.16);  // Crimson Arc — crimson
        palette.primary = vec3(1.00, 0.15, 0.10);    // Crimson Arc — hot red
        palette.contrast = vec3(0.22, 0.44, 1.00);   // Crimson Arc — cobalt split
        palette.accent = vec3(1.00, 0.44, 0.05);     // Crimson Arc — electric orange
        palette.core = vec3(1.00, 0.74, 0.18);       // Crimson Arc — gold
        return palette;
    }

    if (pal == 3) {
        palette.deep = vec3(0.03, 0.12, 0.06);       // Toxic — swamp teal
        palette.secondary = vec3(0.09, 0.52, 0.18);  // Toxic — venom green
        palette.primary = vec3(0.13, 1.00, 0.38);    // Toxic — neon green
        palette.contrast = vec3(0.66, 0.20, 1.00);   // Toxic — ultraviolet
        palette.accent = vec3(0.00, 0.96, 0.82);     // Toxic — acid cyan
        palette.core = vec3(0.60, 1.00, 0.24);       // Toxic — lime spark
        return palette;
    }

    if (pal == 4) {
        palette.deep = vec3(0.03, 0.11, 0.23);       // Frozen — midnight ice
        palette.secondary = vec3(0.12, 0.46, 0.88);  // Frozen — glacier blue
        palette.primary = vec3(0.38, 0.80, 1.00);    // Frozen — ice blue
        palette.contrast = vec3(0.62, 0.52, 1.00);   // Frozen — frost violet
        palette.accent = vec3(0.58, 1.00, 0.92);     // Frozen — mint frost
        palette.core = vec3(0.80, 0.96, 1.00);       // Frozen — pale cyan
        return palette;
    }

    if (pal == 5) {
        palette.deep = vec3(0.04, 0.02, 0.13);       // Phantom — void indigo
        palette.secondary = vec3(0.18, 0.11, 0.46);  // Phantom — indigo
        palette.primary = vec3(0.67, 0.53, 1.00);    // Phantom — spectral lavender
        palette.contrast = vec3(0.12, 0.95, 1.00);   // Phantom — spectral cyan
        palette.accent = vec3(0.95, 0.30, 0.80);     // Phantom — rose flare
        palette.core = vec3(0.42, 0.66, 1.00);       // Phantom — ghost blue
        return palette;
    }

    if (pal == 6) {
        palette.deep = vec3(0.14, 0.02, 0.09);       // Solar Surge — dusk maroon
        palette.secondary = vec3(0.62, 0.07, 0.38);  // Solar Surge — hot rose
        palette.primary = vec3(1.00, 0.24, 0.32);    // Solar Surge — solar red
        palette.contrast = vec3(0.26, 0.30, 1.00);   // Solar Surge — electric blue
        palette.accent = vec3(1.00, 0.55, 0.08);     // Solar Surge — arc amber
        palette.core = vec3(1.00, 0.80, 0.30);       // Solar Surge — bright gold
        return palette;
    }

    palette.deep = vec3(0.07, 0.02, 0.19);           // Rosewire — deep plum
    palette.secondary = vec3(0.24, 0.14, 0.70);      // Rosewire — indigo violet
    palette.primary = vec3(0.96, 0.18, 0.62);        // Rosewire — electric rose
    palette.contrast = vec3(0.12, 1.00, 0.90);       // Rosewire — seafoam neon
    palette.accent = vec3(1.00, 0.42, 0.48);         // Rosewire — coral pink
    palette.core = vec3(1.00, 0.63, 0.84);           // Rosewire — hot blush
    return palette;
}

vec3 sampleArcGradient(ArcPalette palette, float t) {
    float gradientT = saturate(t);

    if (gradientT < 0.26) {
        return mix(palette.deep, palette.secondary, smoothstep(0.0, 1.0, gradientT / 0.26));
    }
    if (gradientT < 0.56) {
        return mix(palette.secondary, palette.primary, smoothstep(0.0, 1.0, (gradientT - 0.26) / 0.30));
    }
    if (gradientT < 0.82) {
        return mix(palette.primary, palette.accent, smoothstep(0.0, 1.0, (gradientT - 0.56) / 0.26));
    }
    return mix(palette.accent, palette.core, smoothstep(0.0, 1.0, (gradientT - 0.82) / 0.18));
}

float contrastWeave(float field, float discharge, float time, float channelShift) {
    float braidA = 0.5 + 0.5 * sin(field * (17.0 + channelShift * 9.0) + time * (2.4 + channelShift * 0.8));
    float braidB = 0.5 + 0.5 * sin(field * (29.0 + channelShift * 11.0) - time * (3.6 + channelShift * 1.1) + channelShift * 18.0);
    float weave = mix(braidA, braidB, 0.42);
    weave = smoothstep(0.70, 0.96, weave);

    // Keep contrast in the branch edges and secondary energy bands, not the hottest core.
    float fringe = smoothstep(0.14, 0.62, discharge) * (1.0 - smoothstep(0.68, 0.94, discharge));
    return weave * fringe;
}

vec3 arcColor(float energy, float field, float channelShift, float time, ArcPalette palette) {
    float discharge = saturate(energy);
    float baseT = saturate(discharge * 0.70 + channelShift);
    float accentT = saturate(0.18 + discharge * 0.62 + channelShift * 0.55);

    vec3 base = sampleArcGradient(palette, baseT);
    vec3 accent = sampleArcGradient(palette, accentT);
    vec3 energized = mix(base, accent, smoothstep(0.36, 0.92, discharge));
    float weave = contrastWeave(field, discharge, time, channelShift);
    float contrastDrift = 0.5 + 0.5 * sin(time * 0.52 + field * 4.4 + channelShift * 9.0);
    vec3 contrastColor = mix(palette.contrast, palette.accent, 0.24 + contrastDrift * 0.28);
    vec3 contrasted = mix(energized, contrastColor, weave * (0.34 + 0.18 * (1.0 - discharge)));

    // Peak energy resolves to the theme's hot tint, while contrast lives in the branch weave.
    return mix(contrasted, palette.core, smoothstep(0.74, 1.0, discharge));
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
    ArcPalette palette = getPalette(iPalette);

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
    vec3 col1 = arcColor(clamp(glow1 * 1.2, 0.0, 1.0), field1, 0.02, time, palette) * glow1;
    vec3 col2 = arcColor(clamp(glow2 * 1.5, 0.0, 1.0), field2, 0.20, time, palette) * glow2;
    vec3 col3 = arcColor(clamp(glow3 * 2.0, 0.0, 1.0), field3, 0.38, time, palette) * glow3;

    // Additive layering — channel crossings intensify into the palette's hot tint
    vec3 col = col1 + col2 + col3;

    // Flash event — brief global intensity spike
    col *= 1.0 + flash;
    col += palette.core * flash * (0.05 + intensity * 0.05);

    // ── Very faint ambient so the background isn't pure black ────
    float ambientDrift = 0.5 + 0.5 * sin(time * 0.34 + p.x * 1.6 - p.y * 1.3);
    vec3 ambientBase = mix(palette.deep, palette.secondary, 0.35);
    vec3 ambient = mix(ambientBase, palette.contrast, ambientDrift * 0.18) * 0.010;
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
