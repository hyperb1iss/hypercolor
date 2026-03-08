#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iSpeed;
uniform float iCloudDensity;
uniform float iWarpStrength;
uniform float iStarField;
uniform float iSaturation;
uniform float iContrast;
uniform int iPalette;

struct NebulaSample {
    vec3 color;
    float alpha;
};

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

float fbm(vec2 p) {
    float sum = 0.0;
    float amp = 0.5;
    for (int i = 0; i < 5; i++) {
        sum += amp * vnoise(p);
        p = p * 2.03 + vec2(9.7, -6.1);
        amp *= 0.5;
    }
    return sum;
}

mat2 rot(float a) {
    float s = sin(a);
    float c = cos(a);
    return mat2(c, -s, s, c);
}

vec3 triGradient(float t, vec3 a, vec3 b, vec3 c) {
    t = fract(t);
    if (t < 0.5) return mix(a, b, t * 2.0);
    return mix(b, c, (t - 0.5) * 2.0);
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return triGradient(t, vec3(0.16, 0.04, 0.24), vec3(0.96, 0.12, 0.74), vec3(0.08, 0.94, 0.92));
    if (id == 1) return triGradient(t, vec3(0.02, 0.10, 0.24), vec3(0.04, 0.84, 1.00), vec3(1.00, 0.14, 0.82));
    if (id == 2) return triGradient(t, vec3(0.02, 0.12, 0.08), vec3(0.04, 0.92, 0.40), vec3(0.54, 0.26, 0.98));
    if (id == 3) return triGradient(t, vec3(0.18, 0.03, 0.01), vec3(0.98, 0.18, 0.04), vec3(1.00, 0.58, 0.08));
    return triGradient(t, vec3(0.10, 0.03, 0.18), vec3(0.96, 0.14, 0.76), vec3(0.30, 0.82, 0.98));
}

vec3 richPaletteColor(float t, int id, float phase) {
    vec3 a = paletteColor(t + phase * 0.05, id);
    vec3 b = paletteColor(t + 0.24 + sin(phase * 1.3) * 0.06, id);
    vec3 c = paletteColor(t + 0.53 + cos(phase * 0.9) * 0.05, id);

    float wa = 0.95 + 0.65 * sin(6.2831853 * (t + phase * 0.09));
    float wb = 0.75 + 0.55 * sin(6.2831853 * (t + 0.31 - phase * 0.05));
    float wc = 0.65 + 0.45 * sin(6.2831853 * (t + 0.63 + phase * 0.04));

    vec3 blended = (a * wa + b * wb + c * wc) / (wa + wb + wc);
    vec3 dominant = a;
    if (wb > wa && wb >= wc) dominant = b;
    if (wc > wa && wc > wb) dominant = c;
    return mix(blended, dominant, 0.34);
}

float starLayer(vec2 uv, float time, float scale, float drift, float amount) {
    vec2 grid = uv * scale + vec2(time * drift, -time * drift * 0.38);
    vec2 cell = floor(grid);
    float seed = hash21(cell);
    if (seed > mix(0.022, 0.008, amount)) return 0.0;

    vec2 local = fract(grid) - 0.5;
    vec2 jitter = hash22(cell + seed * 31.7) - 0.5;
    float dist = length(local - jitter * 0.55);
    float twinkle = 0.55 + 0.45 * sin(time * (1.1 + seed * 2.9) + seed * 74.0);
    return smoothstep(0.08, 0.0, dist) * twinkle;
}

vec3 compositeLayer(vec3 under, vec3 over, float alpha) {
    return mix(under, over, clamp(alpha, 0.0, 1.0));
}

NebulaSample nebulaLayer(vec2 p, float time, float density, float warp, int paletteId, float depth) {
    float zoom = mix(1.1, 2.7, density) * mix(0.78, 1.18, depth);
    vec2 q = p * zoom;
    q += vec2(time * 0.20 * (0.35 + depth), -time * 0.11 * (0.62 - depth * 0.25));
    q = rot(sin(time * 0.12 + depth * 1.7) * 0.32) * q;

    vec2 warpVec = vec2(
        fbm(q * 0.82 + vec2(time * 0.46, -time * 0.28)),
        fbm(rot(0.72) * q * 1.06 - vec2(time * 0.34, time * 0.41))
    ) - 0.5;
    q += warpVec * (0.75 + warp * 1.65) * mix(0.55, 1.0, depth);

    float body = fbm(q * 0.92 + vec2(2.8, -1.6));
    float plume = fbm(q * 1.58 - vec2(time * 0.24, -time * 0.18));
    float ribbon = 1.0 - abs(vnoise(q * 3.6 + vec2(-time * 0.66, time * 0.43)) * 2.0 - 1.0);
    ribbon = pow(clamp(ribbon, 0.0, 1.0), mix(2.2, 4.0, warp));

    float mass = smoothstep(0.24 - density * 0.08, 0.92, body * 0.84 + plume * 0.44 + ribbon * 0.20);
    float haze = smoothstep(0.16, 0.82, body * 0.56 + ribbon * 0.54);
    float streak = smoothstep(0.58, 0.98, ribbon * 0.88 + plume * 0.24);

    float palettePhase = time * 0.16 + body * 0.82 + plume * 0.64 + depth * 1.9;
    vec3 base = richPaletteColor(0.12 + body * 0.34 + warpVec.x * 0.10, paletteId, palettePhase);
    vec3 accent = richPaletteColor(0.40 + plume * 0.26 + ribbon * 0.10, paletteId, palettePhase + 0.22);
    vec3 rim = richPaletteColor(0.70 + ribbon * 0.16 - body * 0.08, paletteId, palettePhase + 0.41);
    vec3 glow = richPaletteColor(0.88 - body * 0.14 + plume * 0.22 + warpVec.y * 0.08, paletteId, palettePhase + 0.63);

    vec3 color = mix(base, accent, clamp(haze * 0.58, 0.0, 0.76));
    color = mix(color, mix(accent, rim, 0.52), clamp(streak * (0.28 + warp * 0.24), 0.0, 0.70));
    color = mix(color, glow, clamp((0.10 + streak * 0.14) * (0.46 + warp * 0.18), 0.0, 0.48));

    float luminance = dot(color, vec3(0.2126, 0.7152, 0.0722));
    color = mix(vec3(luminance), color, 1.16);

    float intensity = mix(0.18, 0.92, mass) * mix(0.84, 1.0, depth);
    float alpha = clamp(
        mass * (0.22 + density * 0.30)
        + haze * 0.16
        + streak * (0.10 + warp * 0.08),
        0.0,
        0.84
    );

    NebulaSample layerResult;
    layerResult.color = color * intensity;
    layerResult.alpha = alpha;
    return layerResult;
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    vec2 p = uv * 2.0 - 1.0;
    p.x *= iResolution.x / iResolution.y;

    float speed = max(iSpeed, 0.2);
    float density = clamp(iCloudDensity * 0.01, 0.10, 1.0);
    float warp = clamp(iWarpStrength * 0.01, 0.0, 1.0);
    float starsAmount = clamp(iStarField * 0.01, 0.0, 1.0);
    float time = iTime * (0.24 + speed * 0.34);

    vec3 bgA = richPaletteColor(0.06 + uv.x * 0.04 + time * 0.02, iPalette, time * 0.20) * 0.06;
    vec3 bgB = richPaletteColor(0.34 + uv.y * 0.08 - time * 0.015, iPalette, time * 0.16 + 0.4) * 0.12;
    vec3 bgC = richPaletteColor(0.72 + uv.x * 0.05 - uv.y * 0.04, iPalette, -time * 0.12 + 0.8) * 0.08;
    vec3 color = mix(bgA, bgB, smoothstep(-0.72, 0.92, uv.y));
    color += bgC * (0.30 + 0.24 * smoothstep(-0.35, 0.85, uv.x - uv.y * 0.2));

    NebulaSample back = nebulaLayer(p * 0.78 + vec2(1.8, -1.2), time * 0.58, density * 0.90, warp * 0.74, iPalette, 0.28);
    NebulaSample mid = nebulaLayer(rot(0.18) * p * 0.96 + vec2(-0.7, 1.1), time * 0.82, density * 0.96, warp * 0.88, iPalette, 0.55);
    NebulaSample front = nebulaLayer(p, time, density, warp, iPalette, 0.86);

    color = compositeLayer(color, back.color, back.alpha * 0.48);
    color = compositeLayer(color, mid.color, mid.alpha * 0.62);
    color = compositeLayer(color, front.color, front.alpha * 0.78);

    float ribbon = 1.0 - abs(vnoise((p + vec2(time * 0.22, -time * 0.17)) * 6.2) * 2.0 - 1.0);
    ribbon = pow(clamp(ribbon, 0.0, 1.0), 5.0);
    vec3 ribbonColor = richPaletteColor(0.66 + ribbon * 0.16, iPalette, time * 0.24 + ribbon * 1.2);
    color = compositeLayer(color, ribbonColor, ribbon * (0.06 + warp * 0.12));

    float stars = 0.0;
    stars += starLayer(uv, time, 120.0, 0.010, starsAmount) * 0.55;
    stars += starLayer(uv, time, 190.0, 0.018, starsAmount) * 0.35;
    stars += starLayer(uv, time, 260.0, 0.028, starsAmount) * 0.18;
    vec3 starTint = mix(vec3(0.20, 0.56, 1.00), richPaletteColor(0.82 + uv.x * 0.08, iPalette, time * 0.18 + 1.1), 0.62);
    color = compositeLayer(color, starTint, stars * (0.08 + starsAmount * 0.10));

    float vignette = smoothstep(1.60, 0.18, length(p));
    color *= 0.40 + 0.92 * vignette;

    color = max(color, vec3(0.0));
    color = 1.0 - exp(-color * (0.94 + warp * 0.18));
    float luminance = dot(color, vec3(0.2126, 0.7152, 0.0722));
    float saturation = clamp(iSaturation * 0.01, 0.0, 1.6);
    float contrast = clamp(iContrast * 0.01, 0.6, 1.5);
    color = mix(vec3(luminance), color, saturation);
    color = (color - 0.5) * contrast + 0.5;
    color = pow(clamp(color, 0.0, 1.0), vec3(0.94));
    fragColor = vec4(color, 1.0);
}
