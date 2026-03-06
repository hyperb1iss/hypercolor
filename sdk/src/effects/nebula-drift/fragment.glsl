#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iSpeed;
uniform float iCloudDensity;
uniform float iWarpStrength;
uniform float iStarField;
uniform int iPalette;

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

vec2 hash22(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * vec3(0.1031, 0.1030, 0.0973));
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.xx + p3.yz) * p3.zy);
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

float fbm4(vec2 p) {
    float sum = 0.0;
    float amp = 0.5;
    for (int i = 0; i < 4; i++) {
        sum += amp * vnoise(p);
        p = p * 2.02 + vec2(13.1, 7.7);
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
    if (id == 0) return triGradient(t, vec3(0.20, 0.05, 0.28), vec3(0.86, 0.18, 0.72), vec3(0.22, 0.84, 0.88));
    if (id == 1) return triGradient(t, vec3(0.04, 0.10, 0.26), vec3(0.16, 0.66, 0.90), vec3(0.92, 0.26, 0.74));
    if (id == 2) return triGradient(t, vec3(0.04, 0.14, 0.08), vec3(0.10, 0.82, 0.46), vec3(0.54, 0.28, 0.90));
    if (id == 3) return triGradient(t, vec3(0.16, 0.04, 0.02), vec3(0.88, 0.18, 0.08), vec3(0.94, 0.48, 0.18));
    return triGradient(t, vec3(0.12, 0.04, 0.16), vec3(0.86, 0.18, 0.70), vec3(0.42, 0.74, 0.90));
}

float starField(vec2 uv, float time, float amount) {
    vec2 grid = uv * mix(vec2(110.0, 70.0), vec2(170.0, 110.0), amount);
    vec2 cell = floor(grid);
    float seed = hash21(cell);
    if (seed > mix(0.028, 0.012, amount)) return 0.0;

    vec2 local = fract(grid) - 0.5;
    vec2 jitter = hash22(cell + seed * 17.0) - 0.5;
    float dist = length(local - jitter * 0.46);
    float twinkle = 0.60 + 0.40 * sin(time * (1.1 + seed * 2.8) + seed * 80.0);
    return smoothstep(0.06, 0.0, dist) * twinkle;
}

vec3 nebulaField(vec2 p, float time, float density, float warp, int paletteId) {
    vec2 q = p * mix(1.4, 2.6, density);
    vec2 flow = vec2(time * 0.05, -time * 0.03);

    float warpA = fbm4(q * 0.95 + vec2(2.4, -1.9) + flow);
    float warpB = fbm4(q * 1.25 + vec2(-3.2, 4.7) - flow * 1.2);
    q += (vec2(warpA, warpB) - 0.5) * (0.35 + warp * 1.35);

    float cloudA = fbm4(q * 0.95 + vec2(0.0, -time * 0.05));
    float cloudB = fbm4(q * 1.85 - vec2(time * 0.08, time * 0.05));
    float wisps = 1.0 - abs(vnoise(q * 4.0 + vec2(time * 0.16, -time * 0.09)) * 2.0 - 1.0);

    float mass = smoothstep(0.42 - density * 0.12, 0.90, cloudA * 0.78 + cloudB * 0.34 + wisps * 0.18);
    float veil = smoothstep(0.28, 0.82, cloudA * 0.55 + wisps * 0.45);
    float accent = smoothstep(0.52, 0.96, cloudB * 0.66 + wisps * 0.38);

    vec3 base = paletteColor(0.18 + cloudA * 0.36 + warpB * 0.18, paletteId);
    vec3 accentCol = paletteColor(0.58 + cloudB * 0.32, paletteId);
    vec3 rim = paletteColor(0.84 + wisps * 0.12, paletteId);

    vec3 col = base * mass * (0.28 + density * 0.54);
    col += accentCol * veil * 0.16;
    col += rim * accent * (0.06 + warp * 0.10);
    return col;
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    vec2 p = uv * 2.0 - 1.0;
    p.x *= iResolution.x / iResolution.y;

    float density = clamp(iCloudDensity * 0.01, 0.10, 1.0);
    float warp = clamp(iWarpStrength * 0.01, 0.0, 1.0);
    float starsAmount = clamp(iStarField * 0.01, 0.0, 1.0);
    float time = iTime * (0.16 + iSpeed * 0.18);

    vec3 bgLow = paletteColor(0.08 + uv.x * 0.05 + time * 0.01, iPalette) * 0.05;
    vec3 bgHigh = paletteColor(0.44 + uv.y * 0.08 - time * 0.008, iPalette) * 0.10;
    vec3 col = mix(bgLow, bgHigh, smoothstep(-0.70, 0.85, uv.y));

    vec3 backLayer = nebulaField(p * 0.82 + vec2(1.9, -1.1), time * 0.70, density * 0.92, warp * 0.75, iPalette);
    vec3 frontLayer = nebulaField(p, time, density, warp, iPalette);
    col += backLayer * 0.56;
    col += frontLayer;

    float dust = vnoise((p + vec2(time * 0.03, -time * 0.02)) * 8.0);
    col += paletteColor(0.44 + dust * 0.20, iPalette) * dust * 0.018 * density;

    float stars = starField(uv, iTime, starsAmount);
    col += vec3(0.54, 0.72, 0.92) * stars * (0.08 + starsAmount * 0.16);

    float vignette = smoothstep(1.55, 0.18, length(p));
    col *= 0.34 + 0.76 * vignette;

    col = max(col, vec3(0.0));
    col = 1.0 - exp(-col * (1.04 + warp * 0.22));
    col = pow(clamp(col, 0.0, 1.0), vec3(0.96));
    fragColor = vec4(col, 1.0);
}
