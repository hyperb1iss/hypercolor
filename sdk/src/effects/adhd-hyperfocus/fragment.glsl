#version 300 es
// Hyperfocus — tunnel vision, dopamine sparks, peripheral fade
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

// Controls — raw values from SDK, normalized below
uniform float iFocusRadius;   // 5-100  -> 0.05-1.0
uniform float iFocusStrength;  // 0-200  -> 0.0-2.0
uniform float iPeripheralBlur; // 0-200  -> 0.0-2.0
uniform float iSaturation;     // 0-200  -> 0.0-2.0
uniform float iEnergy;         // 10-200 -> 0.1-2.0
uniform float iSparkDensity;   // 0-200  -> 0.0-2.0
uniform float iTunnelSpeed;    // 0-200  -> 0.0-2.0
uniform float iParalysis;      // 0-100  -> 0.0-1.0
uniform float iNoise;          // 0-200  -> 0.0-2.0
uniform int iColorMode;
// 0=Dopamine 1=Serotonin 2=Norepinephrine 3=Melatonin 4=Cortisol
// 5=Hyperfocus 6=Bubblegum 7=Neon 8=Void 9=Mono

// ── Helpers ──────────────────────────────────────────────────────

float hash11(float p) {
    p = fract(p * 0.1031);
    p *= p + 33.33;
    p *= p + p;
    return fract(p);
}

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

float vnoise(vec2 x) {
    vec2 i = floor(x);
    vec2 f = fract(x);
    f = f * f * (3.0 - 2.0 * f);
    float a = hash21(i);
    float b = hash21(i + vec2(1.0, 0.0));
    float c = hash21(i + vec2(0.0, 1.0));
    float d = hash21(i + vec2(1.0, 1.0));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

float fbm(vec2 x) {
    float v = 0.0, a = 0.5;
    mat2 rot = mat2(0.8, 0.6, -0.6, 0.8);
    for (int i = 0; i < 4; i++) {
        v += a * vnoise(x);
        x = rot * x * 2.0 + 100.0;
        a *= 0.5;
    }
    return v;
}

// ── Palettes ─────────────────────────────────────────────────────

vec3 mix4(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    float s = fract(t) * 4.0;
    if (s < 1.0) return mix(a, b, s);
    if (s < 2.0) return mix(b, c, s - 1.0);
    if (s < 3.0) return mix(c, d, s - 2.0);
    return mix(d, a, s - 3.0);
}

vec3 palette(float t, int mode) {
    if (mode == 1) {
        // Serotonin — calm teal/seafoam/sky/lavender
        return mix4(t,
            vec3(0.2, 0.8, 0.7),
            vec3(0.4, 0.9, 0.75),
            vec3(0.5, 0.7, 1.0),
            vec3(0.65, 0.5, 0.85)
        );
    } else if (mode == 2) {
        // Norepinephrine — fight-or-flight hot red/orange/white-hot
        return mix4(t,
            vec3(1.0, 0.15, 0.1),
            vec3(1.0, 0.5, 0.1),
            vec3(1.0, 0.9, 0.7),
            vec3(0.8, 0.1, 0.2)
        );
    } else if (mode == 3) {
        // Melatonin — sleepy indigo/midnight/soft purple/dark teal
        return mix4(t,
            vec3(0.2, 0.1, 0.5),
            vec3(0.1, 0.12, 0.35),
            vec3(0.4, 0.2, 0.55),
            vec3(0.1, 0.25, 0.35)
        );
    } else if (mode == 4) {
        // Cortisol — anxious acid green/yellow/amber/sickly
        return mix4(t,
            vec3(0.6, 0.9, 0.1),
            vec3(0.95, 0.9, 0.2),
            vec3(0.9, 0.6, 0.1),
            vec3(0.4, 0.7, 0.2)
        );
    } else if (mode == 5) {
        // Hyperfocus — laser deep blue/cyan/white/blue
        return mix4(t,
            vec3(0.05, 0.1, 0.6),
            vec3(0.0, 0.8, 1.0),
            vec3(0.85, 0.9, 1.0),
            vec3(0.1, 0.2, 0.7)
        );
    } else if (mode == 6) {
        // Bubblegum — playful pink/magenta/lavender/peach
        return mix4(t,
            vec3(1.0, 0.4, 0.6),
            vec3(0.85, 0.2, 0.7),
            vec3(0.75, 0.55, 0.9),
            vec3(1.0, 0.7, 0.6)
        );
    } else if (mode == 7) {
        // Neon — vivid cosine rainbow
        return 0.5 + 0.5 * cos(6.28318 * (t + vec3(0.0, 0.33, 0.67)));
    } else if (mode == 8) {
        // Void — near-black with subtle violet pulses
        return mix4(t,
            vec3(0.05, 0.02, 0.08),
            vec3(0.2, 0.05, 0.25),
            vec3(0.12, 0.1, 0.12),
            vec3(0.05, 0.05, 0.2)
        );
    } else if (mode == 9) {
        // Mono — grayscale
        return vec3(fract(t));
    } else {
        // Dopamine (0) — warm amber/coral/gold with violet punctuation
        return mix4(t,
            vec3(1.0, 0.72, 0.22),
            vec3(1.0, 0.42, 0.38),
            vec3(1.0, 0.85, 0.35),
            vec3(0.45, 0.28, 0.85)
        );
    }
}

// ── Dopamine sparks travelling inward ────────────────────────────

float sparks(vec2 uv, vec2 center, float time, float density, float paralysis) {
    float grid = mix(10.0, 28.0, clamp(density * 0.5, 0.0, 1.0));
    vec2 gid = floor(uv * grid);
    float h = hash21(gid);
    vec2 cellCenter =
        (gid + 0.5 + 0.25 * vec2(hash11(h), hash11(h + 1.7))) / grid;

    float spd = mix(1.6, 0.35, paralysis);
    float life = fract(time * spd + h * 7.3);
    vec2 pos = mix(cellCenter, center, life);

    float r = distance(cellCenter, center);
    float activeMask = step(0.22 + 0.08 * density, r);

    float d = length(uv - pos);
    float glow = exp(-d * mix(150.0, 60.0, density));
    glow *= smoothstep(0.0, 0.2, life) * smoothstep(1.0, 0.6, life);
    return glow * activeMask;
}

// ── Main ─────────────────────────────────────────────────────────

void mainImage(out vec4 color, vec2 fragCoord) {
    float focusR    = iFocusRadius / 100.0;
    float focusStr  = iFocusStrength / 100.0;
    float perBlur   = iPeripheralBlur / 100.0;
    float sat       = iSaturation / 100.0;
    float energy    = iEnergy / 100.0;
    float sparkDens = iSparkDensity / 100.0;
    float tunSpd    = iTunnelSpeed / 100.0;
    float para      = iParalysis / 100.0;
    float noise     = iNoise / 100.0;

    vec2 uv = fragCoord / iResolution.xy;
    vec2 center = vec2(0.5);
    vec2 p = (fragCoord - 0.5 * iResolution.xy) / iResolution.y;

    float t = iTime * (0.2 + 0.8 * tunSpd) * (1.0 - 0.6 * para);

    // Subtle focus center drift — barely perceptible wandering
    float driftAmt = 0.008 * (1.0 - 0.8 * para);
    vec2 drift = vec2(
        sin(t * 0.13) * driftAmt + sin(t * 0.31) * driftAmt * 0.5,
        cos(t * 0.17) * driftAmt + cos(t * 0.29) * driftAmt * 0.4
    );
    vec2 pc = p - drift;

    float r = length(pc);
    float a = atan(pc.y, pc.x);

    // High-frequency noise warping — ripples in rings, not bulk displacement
    float dynamism = 1.0 - 0.5 * para;
    float n1 = fbm(pc * 8.0 + vec2(t * 0.18, t * 0.13)) - 0.5;
    float n2 = fbm(pc * 8.0 + vec2(t * 0.11, -t * 0.15) + 5.2) - 0.5;
    float n3 = fbm(pc * 5.0 + vec2(n1, n2) * 0.4 + t * 0.07) - 0.5;

    float warpedR = r + (n1 * 0.025 + n3 * 0.012) * dynamism;
    float warpedA = a + (n2 * 0.12 + n3 * 0.06) * dynamism;

    // Breathing — whole-field pulsation
    float breath = 1.0 + sin(t * 0.47) * 0.03 + sin(t * 0.83) * 0.015;
    warpedR *= breath;

    // Layered ring frequencies with evolving angular modulation
    float rings1 = sin(12.0 * warpedR - 3.5 * t + sin(warpedA * 2.0 + t * 0.1) * 0.3);
    float rings2 = sin(7.3 * warpedR + 2.1 * t + warpedA * 0.4) * 0.5;
    float rings3 = sin(19.0 * warpedR - 5.2 * t + sin(t * 0.3) * warpedA * 0.5) * 0.25;
    float rings4 = sin(4.7 * warpedR + 1.3 * t + n1 * 3.0) * 0.3;
    float rings = rings1 + rings2 + rings3 + rings4;

    // Base color
    float hueT = fract(0.62 + 0.12 * t + 0.15 * rings + n1 * 0.06);
    vec3 base = palette(hueT, iColorMode);
    base *= energy * 0.7;

    // Focus factor — use raw r for stable focus region
    float focus = smoothstep(focusR, 0.0, r);
    float centerBoost = 1.0 + focusStr * focus;
    base *= centerBoost;

    // Ring structure glow
    float ringBright = pow(max(0.0, rings1), 2.0) * 0.12;
    ringBright += pow(max(0.0, rings2 * 2.0), 3.0) * 0.06;
    ringBright += pow(max(0.0, rings4 * 2.0), 2.0) * 0.04;
    float ringZone = smoothstep(0.0, focusR * 1.3, r) * smoothstep(0.95, focusR * 0.5, r);
    base += palette(hueT + 0.15, iColorMode) * ringBright * ringZone * energy * 0.6;

    // Peripheral desaturation + darkening
    float periph = smoothstep(focusR * 0.8, 0.75, r);
    float periphStrength = clamp(perBlur, 0.0, 2.0);
    float periphFactor = clamp(periph * periphStrength, 0.0, 1.0);

    float satMix = mix(sat, sat * 0.4, periphFactor);
    float l = dot(base, vec3(0.299, 0.587, 0.114));
    base = mix(vec3(l), base, clamp(satMix, 0.0, 2.0));

    float peripheralDim = mix(1.0, 0.15, periphFactor);
    base *= peripheralDim;

    // Film grain noise
    float grain = (vnoise(uv * 800.0 + t * 6.0) - 0.5)
            * noise * (0.4 + 0.6 * periphFactor);
    base += grain;

    // Dopamine sparks — follow drifting center
    vec2 sparkCtr = center + vec2(drift.x * iResolution.y / iResolution.x, drift.y);
    float sp = sparks(uv, sparkCtr, t, sparkDens, para);
    vec3 sparkCol = palette(hueT + 0.1, iColorMode);
    base += sparkCol * sp * (0.7 + 0.6 * sat);

    // Reinhard tone mapping to prevent blowout
    base = base / (1.0 + base * 0.5);
    base = clamp(base, 0.0, 1.0);

    color = vec4(base, 1.0);
}

void main() {
    mainImage(fragColor, gl_FragCoord.xy);
}
