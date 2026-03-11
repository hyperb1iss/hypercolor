#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iSpeed;
uniform float iGridDensity;
uniform float iGlow;
uniform int iScene;
uniform int iPalette;
uniform int iColorMode;
uniform float iCycleSpeed;

vec3 rgb2hsv(vec3 c) {
    vec4 K = vec4(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
    vec4 p = mix(vec4(c.bg, K.wz), vec4(c.gb, K.xy), step(c.b, c.g));
    vec4 q = mix(vec4(p.xyw, c.r), vec4(c.r, p.yzx), step(p.x, c.r));

    float d = q.x - min(q.w, q.y);
    float e = 1.0e-10;
    return vec3(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
}

vec3 hsv2rgb(vec3 c) {
    vec3 p = abs(fract(c.xxx + vec3(0.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0);
    vec3 rgb = clamp(p - 1.0, 0.0, 1.0);
    return c.z * mix(vec3(1.0), rgb, c.y);
}

vec3 hueShift(vec3 color, float shift) {
    vec3 hsv = rgb2hsv(color);
    hsv.x = fract(hsv.x + shift);
    return hsv2rgb(hsv);
}

float lineGrid(float value, float width) {
    float g = abs(fract(value) - 0.5);
    float aa = fwidth(value) * 0.7;
    return 1.0 - smoothstep(width, width + aa, g);
}

float diamondMask(vec2 p, float radius) {
    float d = abs(p.x) + abs(p.y);
    float aa = fwidth(d) * 1.4;
    return 1.0 - smoothstep(radius, radius + aa, d);
}

vec3 paletteColor(int id, int slot) {
    if (id == 0) {
        if (slot == 0) return vec3(0.04, 0.02, 0.09);
        if (slot == 1) return vec3(0.10, 0.02, 0.14);
        if (slot == 2) return vec3(0.88, 0.21, 1.00);
        if (slot == 3) return vec3(0.50, 1.00, 0.92);
        return vec3(1.00, 0.54, 0.18);
    }

    if (id == 1) {
        if (slot == 0) return vec3(0.07, 0.04, 0.02);
        if (slot == 1) return vec3(0.14, 0.05, 0.08);
        if (slot == 2) return vec3(1.00, 0.42, 0.72);
        if (slot == 3) return vec3(0.35, 0.95, 0.55);
        return vec3(1.00, 0.56, 0.22);
    }

    if (id == 2) {
        if (slot == 0) return vec3(0.06, 0.02, 0.00);
        if (slot == 1) return vec3(0.18, 0.03, 0.02);
        if (slot == 2) return vec3(1.00, 0.24, 0.12);
        if (slot == 3) return vec3(1.00, 0.76, 0.14);
        return vec3(0.96, 0.34, 0.66);
    }

    if (id == 3) {
        if (slot == 0) return vec3(0.01, 0.04, 0.08);
        if (slot == 1) return vec3(0.02, 0.08, 0.14);
        if (slot == 2) return vec3(0.22, 0.82, 1.00);
        if (slot == 3) return vec3(0.53, 1.00, 0.86);
        return vec3(0.34, 0.96, 1.00);
    }

    if (slot == 0) return vec3(0.01, 0.01, 0.04);
    if (slot == 1) return vec3(0.06, 0.02, 0.08);
    if (slot == 2) return vec3(0.54, 0.39, 1.00);
    if (slot == 3) return vec3(0.98, 0.27, 0.63);
    return vec3(0.58, 0.91, 1.00);
}

vec3 applyColorMode(vec3 color, int mode, float shift) {
    if (mode == 1) {
        return hueShift(color, shift);
    }

    if (mode == 2) {
        float luma = dot(color, vec3(0.2126, 0.7152, 0.0722));
        return mix(vec3(0.06, 0.10, 0.24), vec3(0.42, 0.98, 0.94), luma);
    }

    return color;
}

vec4 sceneRollerGrid(vec2 p, float t, float density) {
    float horizon = -0.08;
    float floorMask = step(p.y, horizon);
    float floorY = max(0.0, horizon - p.y);
    float depth = 1.0 / (0.09 + floorY);

    float laneScale = mix(3.0, 8.5, density);
    float lane = lineGrid(p.x * depth * laneScale, 0.055);
    float cross = lineGrid((depth + t * 1.3) * (0.75 + density * 2.6), 0.050);
    float primary = max(lane, cross) * floorMask * smoothstep(0.0, 0.85, floorY);

    float side = lineGrid((p.x * depth + sin(depth * 1.7 + t * 1.8) * 0.35) * (1.1 + density * 3.8), 0.030);
    float accent = side * floorMask * (0.35 + 0.65 * smoothstep(0.02, 0.55, floorY));

    float horizonStripe = 1.0 - smoothstep(0.0, 0.012, abs(p.y - horizon));
    float arches = lineGrid(length(vec2(p.x, max(0.0, p.y - horizon) * 1.25)) * (2.8 + density * 6.0) - t * 0.35, 0.046)
        * step(horizon, p.y);

    vec2 tile = vec2(
        p.x * (4.8 + density * 7.2),
        (p.y - horizon) * (3.4 + density * 5.6) + t * 0.25
    );
    vec2 gv = fract(tile) - 0.5;
    float skyDiamond = diamondMask(gv, 0.19) * (1.0 - smoothstep(0.42, 0.95, p.y - horizon));

    float highlight = max(horizonStripe * 1.2, arches * 0.9) + skyDiamond * 0.55;
    float fill = step(horizon, p.y) * (0.18 + 0.22 * lineGrid((p.y - horizon) * (2.5 + density * 4.0), 0.16));

    return vec4(primary, accent + skyDiamond * 0.3, highlight, fill);
}

vec4 sceneArcadeCarpet(vec2 p, float t, float density) {
    float scale = mix(2.4, 7.0, density);
    vec2 q = p * scale + vec2(t * 0.4, -t * 0.28);
    vec2 id = floor(q);
    vec2 gv = fract(q) - 0.5;

    float checker = mod(id.x + id.y, 2.0);
    float diamondRing = smoothstep(0.34, 0.31, abs(gv.x) + abs(gv.y)) - smoothstep(0.20, 0.17, abs(gv.x) + abs(gv.y));
    float boxRing = smoothstep(0.46, 0.43, max(abs(gv.x), abs(gv.y))) - smoothstep(0.28, 0.25, max(abs(gv.x), abs(gv.y)));
    float cross = max(1.0 - smoothstep(0.04, 0.05, abs(gv.x)), 1.0 - smoothstep(0.04, 0.05, abs(gv.y)));

    mat2 rot = mat2(0.70710678, -0.70710678, 0.70710678, 0.70710678);
    vec2 rg = rot * gv;
    float stripeA = lineGrid(rg.x * (4.8 + density * 6.4) + t * 0.45, 0.10);
    float stripeB = lineGrid(rg.y * (4.8 + density * 6.4) - t * 0.45, 0.10);

    float sparkle = diamondMask(gv + vec2(0.0, sin((id.x + id.y) * 0.8 + t) * 0.12), 0.08);

    float primary = mix(diamondRing, boxRing, checker);
    float accent = cross * (0.6 + 0.4 * (1.0 - checker)) + (stripeA * stripeB) * 0.35;
    float highlight = max(stripeA, stripeB) * 0.35 + sparkle * 0.7;

    return vec4(primary, accent, highlight, checker);
}

vec4 sceneLaserLanes(vec2 p, float t, float density) {
    float horizon = -0.18;
    float floorMask = step(p.y, horizon);
    float floorY = max(0.0, horizon - p.y);
    float depth = 1.0 / (0.10 + floorY);

    float laneWarp = sin(depth * 1.6 + t * 0.8) * 0.28;
    float lanesA = lineGrid((p.x * depth + laneWarp) * (1.4 + density * 5.2), 0.040);
    float lanesB = lineGrid((p.x * depth * 0.72 - t * 0.9) * (1.0 + density * 3.6), 0.036);
    float zBeats = lineGrid((depth + t * 1.4) * (0.6 + density * 2.4), 0.050);

    float primary = max(lanesA, zBeats) * floorMask;
    float accent = max(lanesB, (1.0 - smoothstep(0.0, 0.02, abs(p.x))) * zBeats) * floorMask * 0.95;

    float skyMask = step(horizon, p.y);
    float skyBands = lineGrid((p.y - horizon) * (6.0 + density * 9.0) + t * 0.75, 0.070);
    float skyColumns = lineGrid((p.x + sin((p.y - horizon) * 7.0 + t) * 0.15) * (2.0 + density * 3.4), 0.046);
    float arch = lineGrid(length(vec2(p.x, max(0.0, p.y - horizon) * 1.05)) * (3.0 + density * 5.2) - t * 0.32, 0.042)
        * skyMask;

    float highlight = max(arch, skyBands * 0.45 + skyColumns * 0.5);
    float fill = skyMask * (0.15 + 0.35 * skyBands);

    return vec4(primary, accent, highlight, fill);
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    vec2 p = uv * 2.0 - 1.0;
    p.x *= iResolution.x / iResolution.y;

    float density = clamp(iGridDensity / 100.0, 0.0, 1.0);
    float glow = clamp(iGlow / 100.0, 0.0, 1.0);
    float t = iTime * (0.55 + max(iSpeed, 0.05) * 1.2);
    float cycleShift = iTime * (iCycleSpeed * 0.01);

    vec4 scene;
    if (iScene == 0) {
        scene = sceneArcadeCarpet(p, t, density);
    } else if (iScene == 1) {
        scene = sceneLaserLanes(p, t, density);
    } else {
        scene = sceneRollerGrid(p, t, density);
    }

    vec3 bgA = applyColorMode(paletteColor(iPalette, 0), iColorMode, cycleShift);
    vec3 bgB = applyColorMode(paletteColor(iPalette, 1), iColorMode, cycleShift);
    vec3 primary = applyColorMode(paletteColor(iPalette, 2), iColorMode, cycleShift);
    vec3 accent = applyColorMode(paletteColor(iPalette, 3), iColorMode, cycleShift);
    vec3 highlight = applyColorMode(paletteColor(iPalette, 4), iColorMode, cycleShift);

    float baseMix = clamp(scene.w * 0.85 + uv.y * 0.35, 0.0, 1.0);
    vec3 color = mix(bgA, bgB, baseMix);

    float primaryMask = clamp(scene.x, 0.0, 1.2);
    float accentMask = clamp(scene.y, 0.0, 1.2);
    float highlightMask = clamp(scene.z, 0.0, 1.4);

    color += primary * primaryMask * (0.58 + glow * 0.84);
    color += accent * accentMask * (0.56 + glow * 0.88);
    color += highlight * highlightMask * (0.58 + glow * 0.96);

    float bloom = (
        primaryMask * primaryMask * 0.55 +
        accentMask * accentMask * 0.60 +
        highlightMask * highlightMask * 0.80
    ) * glow;
    color += highlight * bloom * 0.34;

    float scan = 0.92 + 0.08 * step(0.5, fract(gl_FragCoord.y * 0.5));
    color *= scan;

    fragColor = vec4(clamp(color, 0.0, 1.0), 1.0);
}
