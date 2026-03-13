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
uniform float iDensity;
uniform float iPrismatic;

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
        palette.deep = vec3(0.04, 0.03, 0.28);       // Electric — midnight indigo
        palette.secondary = vec3(0.18, 0.15, 0.96);  // Electric — violet-blue
        palette.primary = vec3(0.10, 0.92, 1.00);    // Electric — cyan
        palette.contrast = vec3(1.00, 0.18, 0.72);   // Electric — hot magenta
        palette.accent = vec3(1.00, 0.72, 0.40);     // Electric — peach-gold
        palette.core = vec3(0.85, 0.80, 1.00);       // Electric — pale lavender
        return palette;
    }

    if (pal == 1) {
        palette.deep = vec3(0.10, 0.02, 0.22);       // SilkCircuit Storm — deep violet
        palette.secondary = vec3(0.38, 0.10, 1.00);  // SilkCircuit Storm — blue-purple
        palette.primary = vec3(0.88, 0.14, 1.00);    // SilkCircuit Storm — neon magenta
        palette.contrast = vec3(0.08, 1.00, 0.72);   // SilkCircuit Storm — neon mint
        palette.accent = vec3(1.00, 0.35, 0.22);     // SilkCircuit Storm — hot orange
        palette.core = vec3(0.60, 1.00, 0.45);       // SilkCircuit Storm — electric lime
        return palette;
    }

    if (pal == 2) {
        palette.deep = vec3(0.22, 0.02, 0.12);       // Crimson Arc — plum wine
        palette.secondary = vec3(0.85, 0.08, 0.42);  // Crimson Arc — hot magenta
        palette.primary = vec3(1.00, 0.22, 0.05);    // Crimson Arc — solar red-orange
        palette.contrast = vec3(0.15, 0.50, 1.00);   // Crimson Arc — cobalt split
        palette.accent = vec3(1.00, 0.62, 0.08);     // Crimson Arc — electric amber
        palette.core = vec3(1.00, 0.88, 0.55);       // Crimson Arc — warm white-gold
        return palette;
    }

    if (pal == 3) {
        palette.deep = vec3(0.02, 0.10, 0.12);       // Toxic — midnight teal
        palette.secondary = vec3(0.04, 0.52, 0.28);  // Toxic — forest emerald
        palette.primary = vec3(0.45, 1.00, 0.12);    // Toxic — neon chartreuse
        palette.contrast = vec3(0.92, 0.12, 0.72);   // Toxic — hot magenta
        palette.accent = vec3(0.08, 1.00, 0.92);     // Toxic — electric cyan
        palette.core = vec3(0.90, 1.00, 0.18);       // Toxic — neon yellow
        return palette;
    }

    if (pal == 4) {
        palette.deep = vec3(0.03, 0.08, 0.24);       // Frozen — midnight ice
        palette.secondary = vec3(0.22, 0.35, 0.92);  // Frozen — deep periwinkle
        palette.primary = vec3(0.72, 0.55, 1.00);    // Frozen — frost pink-lavender
        palette.contrast = vec3(0.15, 1.00, 0.55);   // Frozen — aurora green
        palette.accent = vec3(0.62, 0.95, 0.85);     // Frozen — warm mint
        palette.core = vec3(0.92, 0.85, 1.00);       // Frozen — pale rose-white
        return palette;
    }

    if (pal == 5) {
        palette.deep = vec3(0.04, 0.02, 0.13);       // Phantom — void indigo
        palette.secondary = vec3(0.12, 0.18, 0.52);  // Phantom — deep teal-indigo
        palette.primary = vec3(0.30, 0.80, 0.85);    // Phantom — spectral teal
        palette.contrast = vec3(1.00, 0.52, 0.40);   // Phantom — rose-gold flare
        palette.accent = vec3(0.88, 0.35, 0.78);     // Phantom — spectral rose
        palette.core = vec3(0.85, 0.82, 0.60);       // Phantom — pale ghost-gold
        return palette;
    }

    if (pal == 6) {
        palette.deep = vec3(0.18, 0.02, 0.15);       // Solar Surge — dusk purple
        palette.secondary = vec3(0.75, 0.05, 0.52);  // Solar Surge — hot magenta
        palette.primary = vec3(1.00, 0.38, 0.08);    // Solar Surge — solar orange
        palette.contrast = vec3(0.08, 0.72, 1.00);   // Solar Surge — electric teal
        palette.accent = vec3(1.00, 0.72, 0.05);     // Solar Surge — hot yellow
        palette.core = vec3(1.00, 0.95, 0.70);       // Solar Surge — warm white
        return palette;
    }

    palette.deep = vec3(0.07, 0.02, 0.19);           // Rosewire — deep plum
    palette.secondary = vec3(0.18, 0.20, 0.75);      // Rosewire — midnight blue
    palette.primary = vec3(0.96, 0.18, 0.62);        // Rosewire — electric rose
    palette.contrast = vec3(0.05, 1.00, 0.65);       // Rosewire — neon green
    palette.accent = vec3(1.00, 0.55, 0.35);         // Rosewire — peach-coral
    palette.core = vec3(1.00, 0.78, 0.62);           // Rosewire — warm gold-blush
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

// ── Zip pulse — traveling intensity bursts along arc geometry ──
// Two counter-propagating waves at different scales create irregular
// bright pulses that race along the ridged-noise tendrils. The field
// value acts as a "distance along the arc" coordinate, so the waves
// track the lightning paths rather than sweeping uniformly.

float zipPulse(float field, vec2 p, float t) {
    float wave1 = sin(field * 26.0 - t * 8.5 + dot(p, vec2(2.7, -3.1)));
    float wave2 = sin(field * 17.0 + t * 5.5 - dot(p, vec2(1.8, 2.4)));
    return smoothstep(0.5, 0.95, max(wave1, wave2));
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

    // Slow color cycling — hues drift through the gradient over time.
    // The field term creates spatial variation so different parts of
    // the arc cycle at different phases, producing flowing color bands.
    float cycleDrift = sin(time * 0.12 + field * 1.8) * 0.14;

    float baseT = saturate(discharge * 0.70 + channelShift + cycleDrift);
    float accentT = saturate(0.18 + discharge * 0.62 + channelShift * 0.55 + cycleDrift * 0.7);

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

    // ── Traveling instability ──────────────────────────────────────
    // Intensity pulses zip along arc paths instead of global flash.
    // Uses field values as spatial coordinates for traveling waves,
    // so bright spots race through the tendrils rather than blanket-blinking.

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

    // ── Density + Prismatic ──────────────────────────────────────
    float density = clamp(iDensity * 0.01, 0.0, 1.0);
    float prismatic = clamp(iPrismatic * 0.01, 0.0, 1.0);

    // ── Inverse-distance glow from ridged fields ─────────────────
    float glowWidth = 0.008 + intensity * 0.024;
    float glowK = 0.015 + intensity * 0.045;

    // Density shifts glow thresholds and widens the catch range.
    // Low density = few crisp bolts, high = dense crackling web.
    float t1 = 0.72 - density * 0.18;
    float t2 = 0.68 - density * 0.16;
    float t3 = 0.65 - density * 0.14;
    float dWidth = glowWidth * (1.0 + density * 0.8);

    // At high density, secondary/tertiary channels catch up to primary
    float ch2w = 0.6 + density * 0.3;
    float ch3w = 0.35 + density * 0.35;

    // ── Prismatic refraction ─────────────────────────────────────
    // Offsets glow thresholds per RGB component so each color "sees"
    // the arc at a slightly different field value. The result is
    // rainbow fringing that follows the arc geometry — like light
    // through a prism. The offset direction slowly rotates so the
    // chromatic split drifts around the arcs over time.
    float prismAmt = prismatic * 0.10;
    float prismPhase = time * 0.18;
    float prismR = prismAmt * sin(prismPhase);
    float prismB = prismAmt * sin(prismPhase + 2.094); // 120° offset

    // Per-channel prismatic glow — vec3(R, G, B) glow intensities
    vec3 pglow1 = vec3(
        glowK / (abs(t1 + prismR - field1) + dWidth),
        glowK / (abs(t1 - field1) + dWidth),
        glowK / (abs(t1 + prismB - field1) + dWidth)
    );
    vec3 pglow2 = vec3(
        glowK * ch2w / (abs(t2 + prismR * 0.8 - field2) + dWidth * 1.2),
        glowK * ch2w / (abs(t2 - field2) + dWidth * 1.2),
        glowK * ch2w / (abs(t2 + prismB * 0.8 - field2) + dWidth * 1.2)
    );
    vec3 pglow3 = vec3(
        glowK * ch3w / (abs(t3 + prismR * 0.6 - field3) + dWidth * 1.5),
        glowK * ch3w / (abs(t3 - field3) + dWidth * 1.5),
        glowK * ch3w / (abs(t3 + prismB * 0.6 - field3) + dWidth * 1.5)
    );

    // Per-channel traveling instability — each arc layer zips independently
    pglow1 *= 1.0 - flicker * 0.50 * zipPulse(field1, p, time);
    pglow2 *= 1.0 - flicker * 0.45 * zipPulse(field2, p, time * 1.2 + 7.0);
    pglow3 *= 1.0 - flicker * 0.40 * zipPulse(field3, p, time * 1.5 + 19.0);

    // ── Localized discharge surge ──────────────────────────────────
    float surgeField = field1 + field2 * 0.5;
    float surgeWave = pow(0.5 + 0.5 * sin(surgeField * 14.0 - time * 6.0), 8.0);
    float surgeTrigger = step(0.85, hash21(vec2(floor(time * 2.0) * 11.7, 42.0)));
    float surge = surgeTrigger * surgeWave * flicker * 0.4;

    // ── Color composition ────────────────────────────────────────
    // Scalar glow (green channel = unshifted) drives the palette lookup.
    // Prismatic blends between uniform glow and per-RGB split glow.
    float sg1 = pglow1.g, sg2 = pglow2.g, sg3 = pglow3.g;

    vec3 tint1 = arcColor(clamp(sg1 * 1.2, 0.0, 1.0), field1, 0.02, time, palette);
    vec3 tint2 = arcColor(clamp(sg2 * 1.5, 0.0, 1.0), field2, 0.20, time, palette);
    vec3 tint3 = arcColor(clamp(sg3 * 2.0, 0.0, 1.0), field3, 0.38, time, palette);

    vec3 col1 = mix(tint1 * sg1, tint1 * pglow1, prismatic);
    vec3 col2 = mix(tint2 * sg2, tint2 * pglow2, prismatic);
    vec3 col3 = mix(tint3 * sg3, tint3 * pglow3, prismatic);

    vec3 col = col1 + col2 + col3;

    // Surge event — localized traveling intensity spike along arc paths
    col += col * surge;
    col += palette.core * surge * 0.05;

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
