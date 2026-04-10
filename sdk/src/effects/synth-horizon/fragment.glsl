#version 300 es
precision highp float;

// ─────────────────────────────────────────────────────────────
// Synth Horizon — chrome sun over a scrolling wireframe grid
// ─────────────────────────────────────────────────────────────
// LED-safe outrun horizon built for spatial resampling:
//   • fwidth-anchored grid AA (no sub-pixel strobe near horizon)
//   • large, bold color fields (sky, sun, mountain silhouettes)
//   • deterministic stars with sinusoidal twinkle (no per-frame noise)
//   • no scanline multiplier (catastrophic when LEDs sample rows)
//
// Six scenes: Open Road, Coastal, Ridge Run, Canyon, Twin Moons, Aurora Peak.
// Five motion styles: Cruise, Serpentine, Solar Pulse, Stargaze, Hyperdrive.
// Twelve palettes. Four color modes incl. Evolve (per-slot hue breathing).

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iSpeed;
uniform float iGlow;
uniform float iSunSize;
uniform float iMountains;
uniform int iPalette;
uniform int iScene;
uniform int iMotion;
uniform int iColorMode;
uniform float iCycleSpeed;

// ── HSV helpers ──────────────────────────────────────────────
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
    return c.z * mix(vec3(1.0), clamp(p - 1.0, 0.0, 1.0), c.y);
}

vec3 hueShift(vec3 color, float shift) {
    vec3 hsv = rgb2hsv(color);
    hsv.x = fract(hsv.x + shift);
    return hsv2rgb(hsv);
}

// Color mode: 0 Static, 1 Color Cycle, 2 Mono Neon, 3 Evolve
// Evolve uses per-slot phase so different elements breathe out of sync.
vec3 applyColorMode(vec3 col, int mode, float shift, int slot) {
    if (mode == 1) return hueShift(col, shift);
    if (mode == 2) {
        float luma = dot(col, vec3(0.2126, 0.7152, 0.0722));
        return mix(vec3(0.04, 0.08, 0.22), vec3(0.48, 1.00, 0.94), luma);
    }
    if (mode == 3) {
        // Evolve keeps breathing even at cycleSpeed 0
        float evolveT = iTime * max(iCycleSpeed * 0.008, 0.09);
        float slotPhase = float(slot) * 1.13;
        float amount = sin(evolveT * 2.6 + slotPhase) * 0.095;
        return hueShift(col, amount);
    }
    return col;
}

// ── Palette (7 slots × 12 palettes) ──────────────────────────
// slot 0 = sky zenith, 1 = sky mid, 2 = horizon glow,
// 3 = sun core, 4 = grid neon, 5 = back mountain, 6 = front mountain
vec3 paletteColor(int id, int slot) {
    // 0 — Nightcall (Kavinsky neon-noir)
    if (id == 0) {
        if (slot == 0) return vec3(0.039, 0.016, 0.125);
        if (slot == 1) return vec3(0.227, 0.043, 0.361);
        if (slot == 2) return vec3(1.000, 0.161, 0.459);
        if (slot == 3) return vec3(1.000, 0.827, 0.098);
        if (slot == 4) return vec3(0.949, 0.133, 1.000);
        if (slot == 5) return vec3(0.071, 0.020, 0.145);
        return vec3(0.020, 0.006, 0.055);
    }
    // 1 — Dusk Drive (Timecop1983 warm cruise)
    if (id == 1) {
        if (slot == 0) return vec3(0.078, 0.039, 0.157);
        if (slot == 1) return vec3(0.290, 0.102, 0.361);
        if (slot == 2) return vec3(0.910, 0.353, 0.561);
        if (slot == 3) return vec3(0.953, 0.808, 0.459);
        if (slot == 4) return vec3(0.016, 0.769, 0.792);
        if (slot == 5) return vec3(0.039, 0.020, 0.094);
        return vec3(0.012, 0.006, 0.032);
    }
    // 2 — Miami Chrome (Hotline Miami white-hot)
    if (id == 2) {
        if (slot == 0) return vec3(0.051, 0.004, 0.149);
        if (slot == 1) return vec3(0.165, 0.031, 0.275);
        if (slot == 2) return vec3(0.996, 0.427, 0.737);
        if (slot == 3) return vec3(0.996, 0.988, 0.980);
        if (slot == 4) return vec3(0.180, 1.000, 1.000);
        if (slot == 5) return vec3(0.035, 0.004, 0.098);
        return vec3(0.010, 0.000, 0.028);
    }
    // 3 — Tron Coast (cool chrome)
    if (id == 3) {
        if (slot == 0) return vec3(0.008, 0.031, 0.078);
        if (slot == 1) return vec3(0.024, 0.141, 0.455);
        if (slot == 2) return vec3(0.000, 0.639, 0.878);
        if (slot == 3) return vec3(0.878, 0.992, 1.000);
        if (slot == 4) return vec3(0.000, 0.780, 0.980);
        if (slot == 5) return vec3(0.004, 0.024, 0.078);
        return vec3(0.000, 0.012, 0.035);
    }
    // 4 — Blood Dragon (hostile heat)
    if (id == 4) {
        if (slot == 0) return vec3(0.008, 0.024, 0.039);
        if (slot == 1) return vec3(0.039, 0.063, 0.188);
        if (slot == 2) return vec3(1.000, 0.090, 0.267);
        if (slot == 3) return vec3(1.000, 0.808, 0.000);
        if (slot == 4) return vec3(0.000, 0.898, 1.000);
        if (slot == 5) return vec3(0.004, 0.016, 0.039);
        return vec3(0.000, 0.008, 0.018);
    }
    // 5 — Vapor Sunset (vaporwave pastel)
    if (id == 5) {
        if (slot == 0) return vec3(0.102, 0.043, 0.251);
        if (slot == 1) return vec3(0.439, 0.247, 1.000);
        if (slot == 2) return vec3(1.000, 0.443, 0.808);
        if (slot == 3) return vec3(1.000, 0.984, 0.588);
        if (slot == 4) return vec3(0.004, 0.804, 0.996);
        if (slot == 5) return vec3(0.063, 0.027, 0.188);
        return vec3(0.020, 0.006, 0.070);
    }
    // 6 — Midnight Coast (FM-84 cold ocean)
    if (id == 6) {
        if (slot == 0) return vec3(0.008, 0.039, 0.110);
        if (slot == 1) return vec3(0.075, 0.278, 0.490);
        if (slot == 2) return vec3(0.361, 0.173, 0.427);
        if (slot == 3) return vec3(0.827, 0.047, 0.722);
        if (slot == 4) return vec3(0.427, 0.945, 0.847);
        if (slot == 5) return vec3(0.004, 0.020, 0.055);
        return vec3(0.000, 0.008, 0.020);
    }
    // 7 — SilkCircuit (brand default)
    if (id == 7) {
        if (slot == 0) return vec3(0.039, 0.020, 0.082);
        if (slot == 1) return vec3(0.165, 0.059, 0.239);
        if (slot == 2) return vec3(0.882, 0.208, 1.000);
        if (slot == 3) return vec3(1.000, 0.416, 0.757);
        if (slot == 4) return vec3(0.502, 1.000, 0.918);
        if (slot == 5) return vec3(0.102, 0.031, 0.161);
        return vec3(0.020, 0.008, 0.059);
    }
    // 8 — Rose Gold (pink, cream, champagne)
    if (id == 8) {
        if (slot == 0) return vec3(0.118, 0.043, 0.094);
        if (slot == 1) return vec3(0.427, 0.125, 0.247);
        if (slot == 2) return vec3(1.000, 0.557, 0.604);
        if (slot == 3) return vec3(1.000, 0.882, 0.702);
        if (slot == 4) return vec3(1.000, 0.733, 0.627);
        if (slot == 5) return vec3(0.086, 0.031, 0.078);
        return vec3(0.035, 0.012, 0.035);
    }
    // 9 — Toxic Rain (acid green + magenta)
    if (id == 9) {
        if (slot == 0) return vec3(0.012, 0.063, 0.039);
        if (slot == 1) return vec3(0.039, 0.184, 0.094);
        if (slot == 2) return vec3(0.867, 0.149, 0.886);
        if (slot == 3) return vec3(0.690, 1.000, 0.278);
        if (slot == 4) return vec3(0.251, 1.000, 0.541);
        if (slot == 5) return vec3(0.008, 0.047, 0.027);
        return vec3(0.000, 0.020, 0.012);
    }
    // 10 — Arctic Mirage (ice blue, white, violet)
    if (id == 10) {
        if (slot == 0) return vec3(0.039, 0.055, 0.133);
        if (slot == 1) return vec3(0.196, 0.259, 0.494);
        if (slot == 2) return vec3(0.710, 0.561, 1.000);
        if (slot == 3) return vec3(0.949, 0.992, 1.000);
        if (slot == 4) return vec3(0.549, 0.933, 1.000);
        if (slot == 5) return vec3(0.031, 0.047, 0.098);
        return vec3(0.008, 0.020, 0.055);
    }
    // 11 — Sunset Strip (orange, purple, yellow)
    if (slot == 0) return vec3(0.086, 0.020, 0.161);
    if (slot == 1) return vec3(0.510, 0.118, 0.427);
    if (slot == 2) return vec3(1.000, 0.431, 0.153);
    if (slot == 3) return vec3(1.000, 0.843, 0.235);
    if (slot == 4) return vec3(0.949, 0.196, 0.686);
    if (slot == 5) return vec3(0.051, 0.020, 0.098);
    return vec3(0.020, 0.008, 0.047);
}

// ── Stable hash for stars and meteors ────────────────────────
float hash12(vec2 v) {
    vec3 p3 = fract(vec3(v.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// ── Scene configuration ──────────────────────────────────────
struct SceneConfig {
    float horizonY;      // y of horizon in aspect-corrected space
    vec2 sunCenter;      // primary sun position
    float sunRadius;     // base radius before sunSize control
    float mountainBias;  // mountain prominence multiplier
    float gridStrength;  // grid brightness multiplier
    float starBias;      // star density bias
    float auroraAmount;  // aurora curtain intensity (0 = off)
    float canyonWalls;   // canyon wall darkening (0 = off)
    float twinMoons;     // twin moons flag (0 or 1)
};

SceneConfig getScene(int id) {
    // 0 — Open Road
    if (id == 0) return SceneConfig(-0.18, vec2(0.00, 0.05), 0.52, 0.55, 1.00, 0.55, 0.0, 0.0, 0.0);
    // 1 — Coastal
    if (id == 1) return SceneConfig(-0.22, vec2(0.00, 0.12), 0.60, 0.00, 0.88, 0.85, 0.0, 0.0, 0.0);
    // 2 — Ridge Run
    if (id == 2) return SceneConfig(-0.10, vec2(0.00, 0.00), 0.46, 1.15, 1.00, 0.40, 0.0, 0.0, 0.0);
    // 3 — Canyon
    if (id == 3) return SceneConfig(-0.14, vec2(0.00, 0.02), 0.42, 1.10, 0.90, 0.50, 0.0, 1.0, 0.0);
    // 4 — Twin Moons
    if (id == 4) return SceneConfig(-0.16, vec2(0.00, 0.12), 0.32, 0.00, 0.85, 0.90, 0.0, 0.0, 1.0);
    // 5 — Aurora Peak
    return SceneConfig(-0.12, vec2(0.00, 0.02), 0.38, 1.05, 0.95, 0.70, 1.0, 0.0, 0.0);
}

// ── Sky gradient (three stops) ───────────────────────────────
vec3 renderSky(vec2 p, float horizonY, vec3 zenith, vec3 mid, vec3 glow) {
    float t = clamp((p.y - horizonY) / (1.05 - horizonY), 0.0, 1.0);
    vec3 col = mix(glow * 0.55, mid, smoothstep(0.0, 0.32, t));
    col = mix(col, zenith, smoothstep(0.38, 1.0, t));
    return col;
}

// ── Ground base tint ─────────────────────────────────────────
vec3 renderGround(vec2 p, float horizonY, vec3 zenith, vec3 glow) {
    float t = clamp((horizonY - p.y) / (1.0 + horizonY), 0.0, 1.0);
    return mix(glow * 0.08, zenith * 0.35, smoothstep(0.0, 0.28, t));
}

// ── Stars (position-hashed, sin-phase twinkle, drift offset) ─
float renderStars(vec2 p, float time, float density, float drift) {
    float topMask = smoothstep(0.05, 0.55, p.y);
    if (topMask < 0.001) return 0.0;

    float total = 0.0;
    for (int layer = 0; layer < 3; layer++) {
        float scale = 26.0 + float(layer) * 22.0;
        // Closer star layers drift faster (parallax hint)
        float layerDrift = drift * (1.0 + float(layer) * 0.4);
        vec2 sp = p * scale + vec2(float(layer) * 7.7 + layerDrift, float(layer) * 3.3);
        vec2 id = floor(sp);
        vec2 fp = fract(sp) - 0.5;
        float h = hash12(id + vec2(float(layer) * 17.3, 0.0));
        float threshold = mix(0.990, 0.955, density) + float(layer) * 0.003;
        if (h < threshold) continue;
        float phase = h * 6.2832;
        float twinkle = 0.35 + 0.65 * (0.5 + 0.5 * sin(time * (0.7 + h * 1.4) + phase));
        float d = length(fp);
        float star = (1.0 - smoothstep(0.02, 0.16, d)) * twinkle;
        total += star * (1.0 - float(layer) * 0.22);
    }
    return total * topMask;
}

// ── Sun with banded chrome stripes + scrollable bands ────────
vec4 renderSun(vec2 p, vec2 center, float radius, vec3 coreCol, vec3 rimCol, float bandOffset) {
    vec2 d = (p - center) * vec2(1.0, 1.04);
    float dist = length(d);
    float sd = dist / max(radius, 0.001);

    float edge = fwidth(sd) * 1.5 + 0.006;
    float core = 1.0 - smoothstep(1.0 - edge, 1.0 + edge, sd);

    float aura = exp(-max(sd - 1.0, 0.0) * 3.6) * 0.40;

    // Vertical pos in sun: -1 top, +1 bottom
    float bandT = (center.y - p.y) / max(radius, 0.001);

    // bandOffset scrolls stripes vertically (Solar Pulse / Hyperdrive)
    float bandCoord = bandT * 3.2 + smoothstep(-0.3, 1.0, bandT) * 3.6 + bandOffset;
    float bandEdge = fract(bandCoord);
    float bandWidth = 0.54;
    float stripe = smoothstep(bandWidth - 0.03, bandWidth + 0.03, bandEdge);
    float bandFade = smoothstep(-0.12, 0.70, bandT);
    float bands = stripe * bandFade;

    float shape = core * (1.0 - bands * 0.94);

    float gradT = clamp(smoothstep(-0.9, 0.95, bandT), 0.0, 1.0);
    vec3 sunCol = mix(coreCol, rimCol, gradT);

    return vec4(sunCol, shape + aura * 0.45);
}

// ── 1D fBm ridge heightmap ───────────────────────────────────
float ridgeNoise(float x, float seed) {
    float h = 0.0;
    float amp = 0.5;
    float freq = 1.4;
    for (int i = 0; i < 4; i++) {
        float s = sin(x * freq + seed);
        h += amp * (1.0 - abs(s));
        freq *= 2.03;
        amp *= 0.55;
    }
    return h;
}

// ── Two-layer mountain silhouette ────────────────────────────
struct MountainResult {
    float backMask;
    float frontMask;
    float ridgeGlow;
};

MountainResult renderMountains(vec2 p, float horizonY, float bias, float drift) {
    MountainResult r;
    r.backMask = 0.0;
    r.frontMask = 0.0;
    r.ridgeGlow = 0.0;
    if (bias <= 0.001) return r;

    float backX = p.x * 1.05 + drift * 0.04;
    float backPeak = 0.06 + ridgeNoise(backX, 1.7) * 0.18 * bias;
    float backTop = horizonY + backPeak;

    float frontX = p.x * 1.65 + drift * 0.09 + 0.5;
    float frontPeak = 0.03 + ridgeNoise(frontX, 3.4) * 0.24 * bias;
    float frontTop = horizonY + frontPeak;

    float aa = fwidth(p.y) * 1.5 + 0.003;
    r.backMask = 1.0 - smoothstep(backTop - aa, backTop + aa, p.y);
    r.frontMask = 1.0 - smoothstep(frontTop - aa, frontTop + aa, p.y);

    r.ridgeGlow = exp(-pow((p.y - backTop) * 85.0, 2.0)) * 0.55;
    return r;
}

// ── Perspective grid floor (fwidth AA + sway) ────────────────
float renderGrid(vec2 p, float horizonY, float scroll, float sway) {
    float below = horizonY - p.y;
    if (below <= 0.002) return 0.0;

    float depth = 1.0 / below;
    float gridZ = depth + scroll;

    // Serpentine sway: sinusoidal lateral offset in grid-space that
    // varies with depth, creating a winding-road curve
    float swayOffset = sway * sin(gridZ * 0.42 + scroll * 0.25);
    float gridX = p.x * depth * 1.10 + swayOffset;

    vec2 g = vec2(gridX, gridZ);
    vec2 gd = abs(fract(g) - 0.5);

    vec2 fw = max(fwidth(g) * 1.3, vec2(0.022));
    vec2 lines = 1.0 - smoothstep(vec2(0.0), fw, gd);
    float grid = max(lines.x, lines.y);

    float nearHorizon = smoothstep(0.0, 0.24, below);
    float farFade = 1.0 / (1.0 + depth * depth * 0.016);
    return grid * nearHorizon * farFade;
}

// ── Aurora curtains (sinusoidal color veils in the sky) ─────
// Returns intensity per pixel + a hue-mix factor for color choice.
vec2 renderAurora(vec2 p, float horizonY, float time) {
    float skyT = clamp((p.y - horizonY) / (1.05 - horizonY), 0.0, 1.0);
    if (skyT < 0.08) return vec2(0.0);

    // Two out-of-phase curtain waves plus a vertical drift
    float wave1 = 0.5 + 0.5 * sin(p.x * 2.1 + time * 0.35 + skyT * 1.6);
    float wave2 = 0.5 + 0.5 * sin(p.x * 3.7 - time * 0.22 + skyT * 0.8 + 1.3);
    float curtain = mix(wave1, wave2, 0.45);
    curtain = pow(curtain, 1.8);

    // Aurora lives in the mid-sky band, not at the zenith or horizon
    float vertical = smoothstep(0.10, 0.32, skyT) * (1.0 - smoothstep(0.55, 0.90, skyT));

    // Color phase — hue mixes between two palette stops
    float hueT = 0.5 + 0.5 * sin(p.x * 1.3 + time * 0.15);

    return vec2(curtain * vertical * 0.55, hueT);
}

// ── Canyon walls — horizontal darkening + rim light ─────────
vec3 applyCanyonWalls(vec3 color, vec2 p, float horizonY, vec3 rimCol, float glow) {
    float dist = abs(p.x);
    // Darken the outer horizontal regions
    float open = smoothstep(1.15, 0.50, dist);
    color *= mix(0.14, 1.0, open);
    // Rim glow at the wall edge
    float rim = exp(-pow((dist - 0.92) * 13.0, 2.0));
    // Only rim above the horizon (the wall rises from the ground)
    float aboveGround = smoothstep(horizonY - 0.05, horizonY + 0.05, p.y);
    color += rimCol * rim * aboveGround * (0.18 + glow * 0.22);
    return color;
}

// ── Main ─────────────────────────────────────────────────────
void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    vec2 p = uv * 2.0 - 1.0;
    float aspect = iResolution.x / iResolution.y;
    p.x *= aspect;

    float glow = clamp(iGlow / 100.0, 0.0, 1.0);
    float sunScale = 0.70 + clamp(iSunSize / 100.0, 0.0, 1.0) * 0.70;
    float mtnBias = clamp(iMountains / 100.0, 0.0, 1.0);
    float speed = max(iSpeed, 0.05);
    float t = iTime * (0.35 + speed * 0.65);
    float cycleShift = iTime * (iCycleSpeed * 0.006);

    SceneConfig scene = getScene(iScene);

    // ── Motion style parameters ─────────────────────────────────
    // 0 Cruise, 1 Serpentine, 2 Solar Pulse, 3 Stargaze, 4 Hyperdrive
    float gridSway = 0.0;
    float sunBandOffset = 0.0;
    float sunRadiusMul = 1.0;
    float gridScrollMul = 1.0;
    float horizonMod = 1.0;
    float starDriftX = 0.0;

    if (iMotion == 1) {
        // Serpentine: grid curves like a winding road
        gridSway = 0.45;
    } else if (iMotion == 2) {
        // Solar Pulse: sun bands scroll, sun breathes, horizon throbs
        sunBandOffset = t * 0.80;
        sunRadiusMul = 1.0 + sin(t * 0.70) * 0.035;
        horizonMod = 0.85 + 0.15 * sin(t * 0.90 + 0.4);
    } else if (iMotion == 3) {
        // Stargaze: stars drift, horizon shimmers, subtle band scroll
        starDriftX = t * 0.12;
        horizonMod = 0.70 + 0.30 * sin(t * 1.25);
        sunBandOffset = t * 0.25;
    } else if (iMotion == 4) {
        // Hyperdrive: warp-speed grid, fast sun pulse, horizon flicker
        gridScrollMul = 2.60;
        gridSway = 0.15;
        sunRadiusMul = 1.0 + sin(t * 1.60) * 0.055;
        sunBandOffset = t * 1.20;
        horizonMod = 0.88 + 0.22 * sin(t * 3.20);
    }

    float horizonY = scene.horizonY;
    vec2 sunCtr = scene.sunCenter;
    float sunRadius = scene.sunRadius * sunScale * sunRadiusMul;

    // ── Palette resolution ──────────────────────────────────────
    vec3 zenith      = applyColorMode(paletteColor(iPalette, 0), iColorMode, cycleShift, 0);
    vec3 skyMid      = applyColorMode(paletteColor(iPalette, 1), iColorMode, cycleShift, 1);
    vec3 horizonGlow = applyColorMode(paletteColor(iPalette, 2), iColorMode, cycleShift, 2);
    vec3 sunCore     = applyColorMode(paletteColor(iPalette, 3), iColorMode, cycleShift, 3);
    vec3 gridColor   = applyColorMode(paletteColor(iPalette, 4), iColorMode, cycleShift, 4);
    vec3 mtnBack     = applyColorMode(paletteColor(iPalette, 5), iColorMode, cycleShift, 5);
    vec3 mtnFront    = applyColorMode(paletteColor(iPalette, 6), iColorMode, cycleShift, 6);

    // ── Sky / ground base ───────────────────────────────────────
    vec3 sky = renderSky(p, horizonY, zenith, skyMid, horizonGlow);
    vec3 ground = renderGround(p, horizonY, zenith, horizonGlow);
    float skyMask = smoothstep(horizonY - 0.002, horizonY + 0.002, p.y);
    vec3 color = mix(ground, sky, skyMask);

    // ── Aurora curtains (Aurora Peak scene only) ────────────────
    if (scene.auroraAmount > 0.5) {
        vec2 aurora = renderAurora(p, horizonY, t);
        vec3 auroraColA = mix(gridColor, horizonGlow, 0.35);
        vec3 auroraColB = mix(sunCore, gridColor, 0.45);
        vec3 auroraCol = mix(auroraColA, auroraColB, aurora.y);
        color += auroraCol * aurora.x * skyMask;
    }

    // ── Stars ───────────────────────────────────────────────────
    float stars = renderStars(p, t * 0.35, scene.starBias, starDriftX) * skyMask;
    vec3 starTint = mix(vec3(1.0), horizonGlow, 0.28);
    color += starTint * stars * 0.85;

    // ── Sun(s) ──────────────────────────────────────────────────
    float sunHorizonCut = smoothstep(horizonY - 0.006, horizonY + 0.004, p.y);
    if (scene.twinMoons > 0.5) {
        // Twin Moons: two suns mirrored left/right, swapped colors
        float moonR = sunRadius * 0.78;
        vec2 m1 = vec2(-0.42, horizonY + 0.26);
        vec2 m2 = vec2(0.42, horizonY + 0.22);
        vec4 moon1 = renderSun(p, m1, moonR, sunCore, horizonGlow, sunBandOffset);
        vec4 moon2 = renderSun(p, m2, moonR * 0.88, horizonGlow, sunCore, sunBandOffset * 0.75 + 1.57);
        moon1.a *= sunHorizonCut;
        moon2.a *= sunHorizonCut;
        color = mix(color, moon1.rgb, moon1.a);
        color = mix(color, moon2.rgb, moon2.a);
    } else {
        vec4 sun = renderSun(p, sunCtr, sunRadius, sunCore, horizonGlow, sunBandOffset);
        sun.a *= sunHorizonCut;
        color = mix(color, sun.rgb, sun.a);
    }

    // ── Mountains ───────────────────────────────────────────────
    MountainResult mtn = renderMountains(p, horizonY, scene.mountainBias * mtnBias, t);
    color = mix(color, mtnBack, mtn.backMask * skyMask);
    color = mix(color, mtnFront, mtn.frontMask * skyMask);
    color += horizonGlow * mtn.ridgeGlow * (0.35 + glow * 0.55) * skyMask;

    // ── Hot horizon band ────────────────────────────────────────
    float horizonBand = exp(-pow((p.y - horizonY) * 34.0, 2.0));
    float horizonFree = (1.0 - mtn.backMask) * (1.0 - mtn.frontMask);
    color += horizonGlow * horizonBand * horizonFree * (0.55 + glow * 0.80) * horizonMod;

    // ── Grid floor ──────────────────────────────────────────────
    float grid = renderGrid(p, horizonY, t * 1.25 * gridScrollMul, gridSway) * scene.gridStrength;
    color += gridColor * grid * (0.80 + glow * 0.85);

    // ── Sun bloom ───────────────────────────────────────────────
    if (scene.twinMoons < 0.5) {
        float bloomDist = length((p - sunCtr) * vec2(1.0, 1.15));
        float sunBloom = exp(-bloomDist * 2.2) * glow * 0.22;
        color += sunCore * sunBloom;
    }

    // ── Secondary grid bloom ────────────────────────────────────
    color += gridColor * grid * grid * glow * 0.30;

    // ── Canyon walls (scene-specific horizontal vignette) ───────
    if (scene.canyonWalls > 0.5) {
        color = applyCanyonWalls(color, p, horizonY, horizonGlow, glow);
    }

    // ── Global vignette ─────────────────────────────────────────
    vec2 vc = uv - 0.5;
    float vignette = 1.0 - dot(vc, vc) * 0.55;
    color *= vignette;

    fragColor = vec4(clamp(color, 0.0, 1.0), 1.0);
}
