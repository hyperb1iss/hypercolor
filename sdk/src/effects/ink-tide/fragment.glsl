#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;
uniform int iPalette;
uniform float iSpeed;
uniform float iFlow;
uniform float iTurbulence;
uniform float iSaturation;

// ─── Noise ──────────────────────────────────────────────────────────

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

mat2 rot(float a) {
    float s = sin(a), c = cos(a);
    return mat2(c, -s, s, c);
}

float fbm(vec2 p) {
    float sum = 0.0;
    float amp = 0.5;
    for (int i = 0; i < 5; i++) {
        sum += amp * vnoise(p);
        p = rot(0.55) * p * 2.05 + vec2(1.7, 9.2);
        amp *= 0.48;
    }
    return sum;
}

// Domain warp — fbm(fbm) for organic ink spreading
float warpedField(vec2 st, float t, vec2 seed1, vec2 seed2, float warpStr) {
    vec2 q = vec2(
        fbm(st + seed1 + t * 0.7),
        fbm(st + seed1 + vec2(5.2, 1.3) + t * 0.9)
    );
    vec2 r = vec2(
        fbm(st + warpStr * q + seed2 + t * 0.5),
        fbm(st + warpStr * q + seed2 + vec2(3.1, 7.4) + t * 0.6)
    );
    return fbm(st + warpStr * r);
}

// ─── Palettes — 3 inks + water per theme ────────────────────────────

struct InkTheme {
    vec3 water;
    vec3 ink1;
    vec3 ink2;
    vec3 ink3;
};

InkTheme getTheme(int id) {
    InkTheme t;
    if (id == 1) {
        // Sakura — hot pink, deep magenta, crimson rose over dark plum
        t.water = vec3(0.02, 0.0, 0.025);
        t.ink1  = vec3(1.0, 0.08, 0.45);
        t.ink2  = vec3(0.75, 0.0, 0.5);
        t.ink3  = vec3(0.9, 0.0, 0.25);
    } else if (id == 2) {
        // Poison — acid green, deep emerald, toxic chartreuse over swamp
        t.water = vec3(0.0, 0.015, 0.0);
        t.ink1  = vec3(0.0, 1.0, 0.2);
        t.ink2  = vec3(0.0, 0.6, 0.1);
        t.ink3  = vec3(0.5, 0.9, 0.0);
    } else if (id == 3) {
        // Molten — deep red, pure orange, dark amber over volcanic black
        t.water = vec3(0.02, 0.005, 0.0);
        t.ink1  = vec3(0.85, 0.04, 0.0);
        t.ink2  = vec3(1.0, 0.35, 0.0);
        t.ink3  = vec3(0.8, 0.55, 0.0);
    } else if (id == 4) {
        // Arctic — deep blue, saturated cyan, teal over dark ocean
        t.water = vec3(0.0, 0.008, 0.03);
        t.ink1  = vec3(0.0, 0.25, 1.0);
        t.ink2  = vec3(0.0, 0.8, 0.9);
        t.ink3  = vec3(0.0, 0.5, 0.7);
    } else if (id == 5) {
        // Phantom — deep violet, electric purple, dark orchid over void
        t.water = vec3(0.01, 0.0, 0.02);
        t.ink1  = vec3(0.45, 0.0, 1.0);
        t.ink2  = vec3(0.7, 0.0, 0.85);
        t.ink3  = vec3(0.35, 0.1, 0.8);
    } else {
        // Abyss — teal, deep blue, dark cyan over black water
        t.water = vec3(0.0, 0.005, 0.02);
        t.ink1  = vec3(0.0, 0.55, 0.7);
        t.ink2  = vec3(0.05, 0.2, 0.9);
        t.ink3  = vec3(0.0, 0.7, 0.5);
    }
    return t;
}

// ─── Main ───────────────────────────────────────────────────────────

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    vec2 p = uv * 2.0 - 1.0;
    p.x *= iResolution.x / iResolution.y;

    float speed = max(iSpeed, 0.2);
    float flow = clamp(iFlow * 0.01, 0.0, 1.0);
    float turb = clamp(iTurbulence * 0.01, 0.0, 1.0);
    float sat = clamp(iSaturation * 0.01, 0.0, 1.0);

    float time = iTime * (0.04 + speed * 0.025);

    float scale = mix(1.2, 2.8, turb);
    vec2 st = p * scale;
    float warpStr = mix(1.5, 5.0, flow);

    // ── Three independent ink fields ──
    // Each has different spatial seeds and time offsets so they move independently
    float fieldA = warpedField(st, time, vec2(0.0, 0.0), vec2(1.7, 9.2), warpStr);
    float fieldB = warpedField(st, time * 1.15, vec2(3.8, 7.1), vec2(6.3, 2.8), warpStr);
    float fieldC = warpedField(st, time * 0.85, vec2(8.5, 4.6), vec2(2.1, 5.7), warpStr);

    // ── Ink concentration — threshold creates distinct fronts ──
    // flow raises the threshold → more ink coverage
    float thresh = mix(0.55, 0.3, flow);
    float edge = mix(0.15, 0.25, turb); // softness of the ink edge

    float inkA = smoothstep(thresh, thresh + edge, fieldA);
    float inkB = smoothstep(thresh + 0.04, thresh + 0.04 + edge, fieldB);
    float inkC = smoothstep(thresh + 0.08, thresh + 0.08 + edge, fieldC);

    // Tendrils at ink fronts — steeper gradient = sharper tendril
    float tendrilA = smoothstep(thresh - 0.02, thresh + 0.04, fieldA)
                   - smoothstep(thresh + 0.04, thresh + edge * 0.7, fieldA);
    float tendrilB = smoothstep(thresh + 0.02, thresh + 0.08, fieldB)
                   - smoothstep(thresh + 0.08, thresh + edge * 0.7 + 0.04, fieldB);

    // ── Compose over dark water ──
    InkTheme theme = getTheme(iPalette);
    vec3 color = theme.water;

    // Layer inks — each paints over with its own concentration as alpha
    color = mix(color, theme.ink1, inkA * 0.95);
    color = mix(color, theme.ink2, inkB * 0.9);
    color = mix(color, theme.ink3, inkC * 0.85);

    // Tendrils at ink fronts — mix, don't add
    color = mix(color, theme.ink1, tendrilA * 0.5);
    color = mix(color, theme.ink2, tendrilB * 0.4);

    // ── Overlap zones — subtractive ink mixing ──
    // Real inks darken when they overlap — multiply the colors together
    float overlapAB = inkA * inkB;
    float overlapBC = inkB * inkC;
    float overlapAC = inkA * inkC;

    // Subtractive: multiply inks together (darkens, shifts hue)
    vec3 mixAB = theme.ink1 * theme.ink2 * 2.0; // *2 to keep it visible (pure multiply is too dark)
    vec3 mixBC = theme.ink2 * theme.ink3 * 2.0;
    vec3 mixAC = theme.ink1 * theme.ink3 * 2.0;

    color = mix(color, mixAB, overlapAB * 0.4);
    color = mix(color, mixBC, overlapBC * 0.35);
    color = mix(color, mixAC, overlapAC * 0.3);

    // Triple overlap — deepest, richest tone (NOT white)
    float tripleOverlap = inkA * inkB * inkC;
    vec3 deepMix = (theme.ink1 + theme.ink2 + theme.ink3) * 0.4;
    color = mix(color, deepMix, tripleOverlap * 0.5);

    // ── Internal variation — subtle luminance ripple within ink bodies ──
    float internal = vnoise(st * 3.0 + time * 2.0);
    float inAnyInk = max(max(inkA, inkB), inkC);
    color *= 0.92 + internal * 0.12 * inAnyInk;

    // ── Saturation — gentle push, never above 1.2x ──
    float lum = dot(color, vec3(0.2126, 0.7152, 0.0722));
    color = mix(vec3(lum), color, 0.7 + sat * 0.5);

    // ── Gentle vignette ──
    float vignette = smoothstep(1.6, 0.2, length(p));
    color *= 0.85 + 0.15 * vignette;

    // ── Output ──
    fragColor = vec4(clamp(color, 0.0, 1.0), 1.0);
}
