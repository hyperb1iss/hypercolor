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

const float PI = 3.14159265359;

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
        // Serotonin — oceanic warmth: deep teal → bright seafoam → periwinkle → soft violet
        return mix4(t,
            vec3(0.15, 0.75, 0.7),
            vec3(0.35, 0.95, 0.8),
            vec3(0.45, 0.55, 1.0),
            vec3(0.7, 0.4, 0.85)
        );
    } else if (mode == 2) {
        // Norepinephrine — fight-or-flight: blood red → hot orange → searing gold → dark crimson
        return mix4(t,
            vec3(1.0, 0.08, 0.05),
            vec3(1.0, 0.45, 0.0),
            vec3(1.0, 0.82, 0.35),
            vec3(0.75, 0.05, 0.2)
        );
    } else if (mode == 3) {
        // Melatonin — sleepy: deep indigo → midnight blue → twilight purple → deep teal
        return mix4(t,
            vec3(0.22, 0.08, 0.52),
            vec3(0.08, 0.12, 0.35),
            vec3(0.45, 0.18, 0.62),
            vec3(0.08, 0.25, 0.35)
        );
    } else if (mode == 4) {
        // Cortisol — anxious: acid green → warning yellow → danger amber → sickly green
        return mix4(t,
            vec3(0.5, 0.95, 0.05),
            vec3(1.0, 0.9, 0.1),
            vec3(0.95, 0.5, 0.0),
            vec3(0.3, 0.75, 0.15)
        );
    } else if (mode == 5) {
        // Hyperfocus — laser: deep electric blue → pure cyan → ice blue → royal blue
        return mix4(t,
            vec3(0.05, 0.12, 0.6),
            vec3(0.0, 0.75, 1.0),
            vec3(0.55, 0.82, 1.0),
            vec3(0.1, 0.18, 0.72)
        );
    } else if (mode == 6) {
        // Bubblegum — playful: hot pink → deep magenta → electric lavender → warm peach
        return mix4(t,
            vec3(1.0, 0.3, 0.55),
            vec3(0.9, 0.15, 0.75),
            vec3(0.7, 0.45, 0.95),
            vec3(1.0, 0.6, 0.5)
        );
    } else if (mode == 7) {
        // Neon — curated electric: neon pink → neon green → electric blue → neon yellow
        return mix4(t,
            vec3(1.0, 0.0, 0.4),
            vec3(0.0, 1.0, 0.6),
            vec3(0.0, 0.4, 1.0),
            vec3(1.0, 0.9, 0.0)
        );
    } else if (mode == 8) {
        // Void — abyss with presence: muted violet → deep violet pulse → twilight haze → deep indigo
        return mix4(t,
            vec3(0.06, 0.02, 0.1),
            vec3(0.25, 0.06, 0.32),
            vec3(0.1, 0.08, 0.14),
            vec3(0.05, 0.04, 0.22)
        );
    } else if (mode == 9) {
        // Mono — cool silver gradient with depth
        return mix4(t,
            vec3(0.9, 0.92, 0.95),
            vec3(0.2, 0.21, 0.25),
            vec3(0.55, 0.58, 0.65),
            vec3(0.08, 0.08, 0.12)
        );
    } else {
        // Dopamine (0) — reward circuit: rich gold → hot coral → electric violet → warm amber
        return mix4(t,
            vec3(1.0, 0.78, 0.2),
            vec3(1.0, 0.35, 0.3),
            vec3(0.7, 0.25, 0.9),
            vec3(1.0, 0.55, 0.15)
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

    // Layered ring frequencies with seam-safe angular modulation.
    // Any raw `atan` branch cut lives on the left edge, so angular terms
    // must stay 2π-periodic in the original angle to avoid a visible pinch.
    float angularFlow = sin(warpedA + t * 0.21 + n2 * 1.4);
    float angularPetals = sin(3.0 * warpedA - t * 0.16 + n3 * 1.8);
    float angularLattice = cos(2.0 * warpedA + t * 0.12 + n1 * 2.2);

    float rings1 = sin(12.0 * warpedR - 3.5 * t + angularLattice * 0.32);
    float rings2 = sin(7.3 * warpedR + 2.1 * t + angularFlow * 0.75 + angularPetals * 0.25) * 0.5;
    float rings3 = sin(17.0 * warpedR - 4.4 * t + angularPetals * 0.7 + angularLattice * 0.3) * 0.24;
    float rings4 = sin(5.4 * warpedR + 1.8 * t + angularFlow * 1.2 + n1 * 2.4) * 0.32;
    float ringPulse = sin(9.2 * warpedR - 2.6 * t + angularPetals * 1.15 - angularFlow * 0.55);
    float rings = rings1 + rings2 + rings3 + rings4 + ringPulse * 0.18;

    // Base color with luminance-aware boost for dark palettes
    float hueT = fract(0.62 + 0.12 * t + 0.15 * rings + n1 * 0.06);
    vec3 base = palette(hueT, iColorMode);
    float baseLum = dot(base, vec3(0.299, 0.587, 0.114));
    base *= mix(1.5, 1.0, smoothstep(0.15, 0.5, baseLum));
    base *= energy * 0.82;

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

    float haloRadius = focusR * (1.02 + 0.16 * sin(t * 0.9 + angularPetals));
    float haloWidth = mix(0.04, 0.11, focusR);
    float halo = exp(-pow((r - haloRadius) / haloWidth, 2.0));
    float haloPulse = 0.55 + 0.45 * sin(t * 1.35 - warpedR * 13.0 + angularFlow * PI);
    base += palette(hueT + 0.28 + angularFlow * 0.04, iColorMode)
            * halo
            * haloPulse
            * energy
            * (0.08 + 0.10 * focusStr);

    float orbitBand = exp(-pow((r - (focusR * 0.72 + 0.03 * angularFlow + 0.02 * sin(t * 0.6))) / 0.09, 2.0));
    float orbitSpark = pow(max(0.0, 0.5 + 0.5 * sin(2.0 * warpedA + 10.0 * warpedR - 2.2 * t)), 4.0);
    base += palette(hueT + 0.42, iColorMode) * orbitBand * orbitSpark * energy * 0.11 * dynamism;

    // Nebula cloud layer — slow color fog fills dark peripheral regions
    float nebulaMask = smoothstep(0.35, 0.65, n3 + 0.5) * smoothstep(0.3, 0.7, n1 + 0.5);
    vec3 nebulaColor = palette(hueT + 0.25 + n2 * 0.12, iColorMode);
    base += nebulaColor * nebulaMask * energy * 0.15 * (0.3 + 0.7 * (1.0 - focus));

    // Specular highlights on ring crests — palette-independent brightness
    float specHighlight = pow(max(0.0, rings1), 5.0) * 0.06
                        + pow(max(0.0, rings4 * 2.0), 4.0) * 0.03;
    base += vec3(specHighlight) * energy * (0.3 + 0.7 * focus);

    // Chromatic edge accent at focus boundary
    float edgeBand = exp(-pow((r - focusR) / 0.06, 2.0));
    vec3 edgeColor = palette(hueT + 0.5, iColorMode);
    base += edgeColor * edgeBand * energy * 0.12;

    // Peripheral desaturation + darkening
    float periph = smoothstep(focusR * 0.8, 0.75, r);
    float periphStrength = clamp(perBlur, 0.0, 2.0);
    float periphFactor = clamp(periph * periphStrength, 0.0, 1.0);

    float satMix = mix(sat, sat * 0.4, periphFactor);
    float l = dot(base, vec3(0.299, 0.587, 0.114));
    base = mix(vec3(l), base, clamp(satMix, 0.0, 2.0));

    float peripheralDim = mix(1.0, 0.22, periphFactor);
    base *= peripheralDim;

    // Film grain noise
    float grain = (vnoise(uv * 800.0 + t * 6.0) - 0.5)
            * noise * (0.4 + 0.6 * periphFactor);
    base += grain;

    // Dopamine sparks — follow drifting center
    vec2 sparkCtr = center + vec2(drift.x * iResolution.y / iResolution.x, drift.y);
    float sp = sparks(uv, sparkCtr, t, sparkDens, para);
    vec3 sparkCol = palette(hueT + 0.1 + angularFlow * 0.03, iColorMode);
    base += sparkCol * sp * (0.7 + 0.6 * sat);

    // Reinhard tone mapping to prevent blowout
    base = base / (1.0 + base * 0.5);
    base = clamp(base, 0.0, 1.0);

    color = vec4(base, 1.0);
}

void main() {
    mainImage(fragColor, gl_FragCoord.xy);
}
