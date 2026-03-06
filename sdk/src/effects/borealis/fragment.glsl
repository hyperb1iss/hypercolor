#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iSpeed;
uniform float iIntensity;
uniform float iWarpStrength;
uniform float iStarBrightness;
uniform float iCurtainHeight;
uniform int iPalette;

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

float fbm3(vec2 p) {
    float sum = 0.0;
    float amp = 0.5;
    for (int i = 0; i < 3; i++) {
        sum += amp * vnoise(p);
        p = p * 2.02 + vec2(11.7, 6.3);
        amp *= 0.5;
    }
    return sum;
}

vec3 triGradient(float t, vec3 a, vec3 b, vec3 c) {
    t = fract(t);
    if (t < 0.5) return mix(a, b, t * 2.0);
    return mix(b, c, (t - 0.5) * 2.0);
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return triGradient(t, vec3(0.03, 0.16, 0.08), vec3(0.08, 0.86, 0.38), vec3(0.50, 0.20, 0.88));
    if (id == 1) return triGradient(t, vec3(0.10, 0.04, 0.16), vec3(0.86, 0.16, 0.82), vec3(0.18, 0.92, 0.82));
    if (id == 2) return triGradient(t, vec3(0.04, 0.10, 0.22), vec3(0.12, 0.88, 0.70), vec3(0.94, 0.22, 0.72));
    if (id == 3) return triGradient(t, vec3(0.10, 0.04, 0.12), vec3(0.94, 0.34, 0.22), vec3(0.78, 0.16, 0.54));
    if (id == 4) return triGradient(t, vec3(0.03, 0.10, 0.20), vec3(0.18, 0.80, 0.84), vec3(0.40, 0.40, 0.90));
    if (id == 5) return triGradient(t, vec3(0.12, 0.03, 0.02), vec3(0.82, 0.18, 0.05), vec3(0.96, 0.56, 0.08));
    if (id == 6) return triGradient(t, vec3(0.08, 0.04, 0.16), vec3(0.94, 0.24, 0.72), vec3(0.34, 0.84, 0.92));
    return triGradient(t, vec3(0.00, 0.08, 0.02), vec3(0.10, 0.72, 0.24), vec3(0.52, 0.94, 0.34));
}

float starField(vec2 uv, float time) {
    vec2 grid = uv * vec2(150.0, 95.0);
    vec2 cell = floor(grid);
    float seed = hash21(cell);
    if (seed > 0.024) return 0.0;

    vec2 local = fract(grid) - 0.5;
    vec2 jitter = vec2(hash21(cell + 1.7), hash21(cell + 9.2)) - 0.5;
    float dist = length(local - jitter * 0.44);
    float twinkle = 0.65 + 0.35 * sin(time * (1.2 + seed * 2.8) + seed * 70.0);
    return smoothstep(0.06, 0.0, dist) * twinkle;
}

vec3 auroraLayer(vec2 p, float time, float layer, float baseHeight) {
    float depth = layer * 0.32;
    float warpStrength = 0.20 + iWarpStrength * 0.010;

    vec2 q = p;
    q.x *= 1.16 + depth * 0.34;
    q.y *= 0.72 + depth * 0.08;
    q += vec2(depth * 1.7 - time * (0.06 + depth * 0.04), depth * 0.14);

    float warpA = fbm3(q * (0.95 + depth * 0.18) + vec2(0.0, time * 0.06));
    float warpB = vnoise(q * (1.28 + depth * 0.22) + vec2(4.1, -3.7) - vec2(time * 0.04));
    vec2 warped = q + (vec2(warpA, warpB) - 0.5) * warpStrength;

    float sweep = sin(warped.x * (2.6 + depth * 0.6) + time * (0.8 + depth * 0.22) + warpA * 4.8);
    float ridge = baseHeight + (warpB - 0.5) * (0.52 - depth * 0.10) + sweep * (0.08 + depth * 0.03);
    float drop = ridge - p.y;

    float curtain = smoothstep(-0.04, 0.16, drop) * (1.0 - smoothstep(0.78, 1.08, drop));
    float folds = vnoise(vec2(warped.x * 3.4 + layer * 1.8, p.y * 1.5 - time * 0.08));
    float filaments = 0.55 + 0.45 * sin(warped.x * 11.0 + folds * 5.0 + time * 1.12 + layer * 1.9);
    filaments *= 0.78 + 0.22 * sin(warped.x * 22.0 - time * 0.48 + layer * 3.1);
    float beam = curtain * mix(0.42, 1.0, filaments);

    float body = smoothstep(0.12, 0.88, drop) * (1.0 - smoothstep(0.88, 1.22, drop));
    float ribbon = smoothstep(0.04, 0.40, drop) * (1.0 - smoothstep(0.40, 0.76, drop));
    float crown = smoothstep(-0.03, 0.22, drop) * (1.0 - smoothstep(0.22, 0.48, drop));

    vec3 col = paletteColor(0.16 + layer * 0.10 + warpA * 0.42 + filaments * 0.15, iPalette);
    if (iPalette == 0) {
        vec3 greenCore = vec3(0.04, 0.94, 0.40);
        vec3 emerald = vec3(0.10, 0.76, 0.34);
        vec3 magenta = vec3(0.86, 0.22, 0.78);
        vec3 violet = vec3(0.44, 0.20, 0.92);

        vec3 physical = emerald * (0.18 + body * 0.62);
        physical += greenCore * (0.10 + body * 0.36 * (0.7 + 0.3 * filaments));
        physical += magenta * ribbon * (0.14 + 0.28 * filaments);
        physical += violet * crown * (0.14 + 0.30 * (1.0 - filaments * 0.4));
        col = mix(col, physical, 0.92);
    }

    float edge = smoothstep(0.02, 0.30, drop) * (1.0 - smoothstep(0.30, 0.56, drop));
    vec3 highlight = (iPalette == 0) ? vec3(0.16, 0.92, 0.54) : paletteColor(0.82 + layer * 0.05, iPalette);
    col = mix(col, highlight, edge * 0.20 * filaments);

    float glow = exp(-abs(drop - 0.18) * 6.4) * 0.20;
    float strength = beam * (0.20 + iIntensity * 0.010) * (1.0 - depth * 0.12);
    strength += glow * 0.04;
    return col * strength;
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    vec2 p = (uv - 0.5) * vec2(iResolution.x / iResolution.y, 1.0);
    float time = iTime * iSpeed * 0.28;

    vec3 skyZenith = vec3(0.004, 0.010, 0.028);
    vec3 skyHorizon = (iPalette == 0)
        ? vec3(0.010, 0.046, 0.030)
        : paletteColor(0.10 + time * 0.01, iPalette) * 0.08;
    float skyMix = smoothstep(-0.48, 0.76, p.y);
    vec3 col = mix(skyHorizon, skyZenith, skyMix);

    float lowMist = 1.0 - smoothstep(-0.42, -0.02, p.y);
    col += skyHorizon * lowMist * 0.16;

    float stars = starField(uv, iTime) * (iStarBrightness * 0.010);
    col += vec3(0.48, 0.66, 0.86) * stars;

    float baseHeight = mix(-0.04, 0.28, clamp(iCurtainHeight * 0.01, 0.0, 1.0));
    vec3 aurora = vec3(0.0);
    for (int i = 0; i < 3; i++) {
        aurora += auroraLayer(p, time, float(i), baseHeight);
    }

    float horizonGlow = exp(-abs(p.y + 0.18) * 6.2);
    aurora += paletteColor(0.24 + time * 0.01, iPalette) * horizonGlow * 0.03 * (0.35 + iIntensity * 0.010);

    col += aurora;

    if (iPalette == 0) {
        float upperTint = smoothstep(-0.04, 0.92, p.y);
        col += vec3(0.03, 0.08, 0.17) * upperTint * 0.06;
        col += vec3(0.02, 0.10, 0.06) * upperTint * 0.04;
    }

    float groundGlow = 1.0 - smoothstep(-0.54, -0.14, p.y);
    col += aurora * groundGlow * 0.10;

    col = max(col, vec3(0.0));
    col = 1.0 - exp(-col * (1.08 + iIntensity * 0.003));
    col = pow(clamp(col, 0.0, 1.0), vec3(0.95));

    fragColor = vec4(col, 1.0);
}
