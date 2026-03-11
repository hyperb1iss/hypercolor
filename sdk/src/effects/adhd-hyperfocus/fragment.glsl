#version 300 es
// ADHD Hyperfocus — tunnel vision, dopamine sparks, peripheral fade
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
uniform int iColorMode;        // 0=Dopamine, 1=Neon, 2=Mono

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

// Value noise with smooth interpolation
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

vec3 hsv2rgb(vec3 c) {
    vec4 K = vec4(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    vec3 p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
    return c.z * mix(K.xxx, clamp(p - K.xxx, 0.0, 1.0), c.y);
}

// ── Palettes ─────────────────────────────────────────────────────

vec3 palette(float t, int mode) {
    if (mode == 1) {
        // Neon — vivid cosine palette
        return 0.5 + 0.5 * cos(6.28318 * (t + vec3(0.0, 0.33, 0.67)));
    } else if (mode == 2) {
        // Mono — grayscale
        return vec3(t);
    } else {
        // Dopamine — warm reward colors cycling through cool accents
        // Amber/coral/gold base with blue-violet punctuation
        float phase = fract(t);
        // Four-stop gradient: amber -> coral -> gold -> blue-violet -> loop
        vec3 amber  = vec3(1.0, 0.72, 0.22);
        vec3 coral  = vec3(1.0, 0.42, 0.38);
        vec3 gold   = vec3(1.0, 0.85, 0.35);
        vec3 violet = vec3(0.45, 0.28, 0.85);

        float segment = phase * 4.0;
        vec3 c;
        if (segment < 1.0) {
            c = mix(amber, coral, segment);
        } else if (segment < 2.0) {
            c = mix(coral, gold, segment - 1.0);
        } else if (segment < 3.0) {
            c = mix(gold, violet, segment - 2.0);
        } else {
            c = mix(violet, amber, segment - 3.0);
        }
        return c;
    }
}

// ── Dopamine sparks travelling inward ────────────────────────────

float sparks(vec2 uv, vec2 center, float time, float density, float paralysis) {
    float grid = mix(10.0, 28.0, clamp(density * 0.5, 0.0, 1.0));
    vec2 gid = floor(uv * grid);
    float h = hash21(gid);
    vec2 cellCenter =
        (gid + 0.5 + 0.25 * vec2(hash11(h), hash11(h + 1.7))) / grid;

    // Direction toward focus center
    vec2 dir = normalize(center - cellCenter + 1e-4);
    float spd = mix(1.6, 0.35, paralysis);

    // Each cell has a spark lifecycle
    float life = fract(time * spd + h * 7.3);
    vec2 pos = mix(cellCenter, center, life);

    // Only spawn sparks outside the focus radius
    float r = distance(cellCenter, center);
    float activeMask = step(0.22 + 0.08 * density, r);

    float d = length(uv - pos);
    float glow = exp(-d * mix(150.0, 60.0, density));
    glow *= smoothstep(0.0, 0.2, life) * smoothstep(1.0, 0.6, life);
    return glow * activeMask;
}

// ── Main ─────────────────────────────────────────────────────────

void mainImage(out vec4 color, vec2 fragCoord) {
    // Normalize control values from raw SDK ranges
    float focusR    = iFocusRadius / 100.0;     // 0.05 - 1.0
    float focusStr  = iFocusStrength / 100.0;    // 0.0  - 2.0
    float perBlur   = iPeripheralBlur / 100.0;   // 0.0  - 2.0
    float sat       = iSaturation / 100.0;        // 0.0  - 2.0
    float energy    = iEnergy / 100.0;            // 0.1  - 2.0
    float sparkDens = iSparkDensity / 100.0;      // 0.0  - 2.0
    float tunSpd    = iTunnelSpeed / 100.0;       // 0.0  - 2.0
    float para      = iParalysis / 100.0;         // 0.0  - 1.0
    float noise     = iNoise / 100.0;             // 0.0  - 2.0

    vec2 uv = fragCoord / iResolution.xy;
    vec2 center = vec2(0.5);
    vec2 p = (fragCoord - 0.5 * iResolution.xy) / iResolution.y;

    float t = iTime * (0.2 + 0.8 * tunSpd) * (1.0 - 0.6 * para);
    float r = length(p);
    float a = atan(p.y, p.x);

    // Flowing perturbations — noise warps radius for organic, breathing rings
    float warpSlow = vnoise(p * 4.0 + vec2(t * 0.3, t * 0.2)) - 0.5;
    float warpFast = vnoise(p * 9.0 - vec2(t * 0.45, t * 0.15)) - 0.5;
    float warpedR = r + warpSlow * 0.06 + warpFast * 0.025;

    // Angular wobble — rings aren't perfect circles
    float wobble = sin(a * 3.0 + t * 0.7) * 0.035
                 + sin(a * 5.0 - t * 1.1) * 0.018
                 + sin(a * 2.0 + t * 0.4 + r * 4.0) * 0.025;
    warpedR += wobble * (1.0 - 0.5 * para);

    // Layered ring frequencies that interfere — creates complex flowing structure
    float rings1 = sin(12.0 * warpedR - 3.5 * t);
    float rings2 = sin(7.3 * warpedR + 2.1 * t + a * 0.4) * 0.5;
    float rings3 = sin(19.0 * warpedR - 5.2 * t - a * 0.25) * 0.25;
    float rings = rings1 + rings2 + rings3;

    // Base color via palette
    float hueT = fract(0.62 + 0.12 * t + 0.15 * rings);
    vec3 base = palette(hueT, iColorMode);
    base *= energy * 0.7;

    // Focus factor — peaks at center, falls off with radius
    float focus = smoothstep(focusR, 0.0, r);

    // Center boost: center (focus=1) gets boosted, periphery (focus=0) stays at 1.0
    float centerBoost = 1.0 + focusStr * focus;
    base *= centerBoost;

    // Ring structure glow — visible ripples pulling inward
    float ringBright = pow(max(0.0, rings1), 2.0) * 0.12;
    ringBright += pow(max(0.0, rings2 * 2.0), 3.0) * 0.06;
    float ringZone = smoothstep(0.0, focusR * 1.3, r) * smoothstep(0.95, focusR * 0.5, r);
    base += palette(hueT + 0.15, iColorMode) * ringBright * ringZone * energy * 0.6;

    // Peripheral desaturation + darkening
    float periph = smoothstep(focusR * 0.8, 0.75, r);
    float periphStrength = clamp(perBlur, 0.0, 2.0);
    float periphFactor = clamp(periph * periphStrength, 0.0, 1.0);

    // Desaturate the periphery
    float satMix = mix(sat, sat * 0.4, periphFactor);
    float l = dot(base, vec3(0.299, 0.587, 0.114));
    base = mix(vec3(l), base, clamp(satMix, 0.0, 2.0));

    // Darken the periphery — smooth vignette falloff
    float peripheralDim = mix(1.0, 0.15, periphFactor);
    base *= peripheralDim;

    // Film grain noise (stronger at edges)
    float n = (vnoise(uv * 800.0 + t * 6.0) - 0.5)
            * noise * (0.4 + 0.6 * periphFactor);
    base += n;

    // Dopamine sparks
    float sp = sparks(uv, center, t, sparkDens, para);
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
