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
uniform float iSaturation;
uniform float iContrast;
uniform float iBanding;
uniform int iPalette;

// ── Noise ─────────────────────────────────────────────────────

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

// Rotational FBM — inter-octave rotation breaks grid alignment
// for richer, more organic flowing patterns
const mat2 fbmRot = mat2(0.80, 0.60, -0.60, 0.80);

float fbm3(vec2 p) {
    float sum = 0.0, amp = 0.5;
    for (int i = 0; i < 3; i++) {
        sum += amp * vnoise(p);
        p = fbmRot * p * 2.03 + vec2(11.7, 6.3);
        amp *= 0.48;
    }
    return sum;
}

// Ridge noise — folds value noise to create sharp bright crests
float ridgeNoise(vec2 p) {
    return 1.0 - abs(vnoise(p) * 2.0 - 1.0);
}

// ── Palettes ──────────────────────────────────────────────────

vec3 triGradient(float t, vec3 a, vec3 b, vec3 c) {
    t = fract(t);
    if (t < 0.5) return mix(a, b, t * 2.0);
    return mix(b, c, (t - 0.5) * 2.0);
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return triGradient(t, vec3(0.04, 0.10, 0.22), vec3(0.12, 0.88, 0.70), vec3(0.94, 0.22, 0.72));
    if (id == 1) return triGradient(t, vec3(0.12, 0.03, 0.02), vec3(0.82, 0.18, 0.05), vec3(0.96, 0.56, 0.08));
    if (id == 2) return triGradient(t, vec3(0.03, 0.10, 0.20), vec3(0.18, 0.80, 0.84), vec3(0.40, 0.40, 0.90));
    if (id == 3) return triGradient(t, vec3(0.03, 0.16, 0.08), vec3(0.08, 0.86, 0.38), vec3(0.50, 0.20, 0.88));
    if (id == 4) return triGradient(t, vec3(0.00, 0.08, 0.02), vec3(0.10, 0.72, 0.24), vec3(0.52, 0.94, 0.34));
    if (id == 5) return triGradient(t, vec3(0.10, 0.04, 0.16), vec3(0.86, 0.16, 0.82), vec3(0.18, 0.92, 0.82));
    if (id == 6) return triGradient(t, vec3(0.10, 0.04, 0.12), vec3(0.94, 0.34, 0.22), vec3(0.78, 0.16, 0.54));
    return triGradient(t, vec3(0.08, 0.04, 0.16), vec3(0.94, 0.24, 0.72), vec3(0.34, 0.84, 0.92));
}

vec3 richPaletteColor(float t, int id) {
    vec3 base = paletteColor(t, id);
    vec3 accent = paletteColor(t + 0.22, id);
    vec3 rim = paletteColor(t + 0.48, id);
    return base * 0.56 + accent * 0.28 + rim * 0.16;
}

// ── Stars ─────────────────────────────────────────────────────

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

// ── Aurora layer ──────────────────────────────────────────────
// 4 layers with distinct motion and contrast profiles:
//   0: broad background wash (slow, wide, soft)
//   1: main curtain (medium speed, rich detail)
//   2: counter-curtain (drifts opposite direction)
//   3: fast bright filaments (high speed, sharp peaks)

vec3 auroraLayer(vec2 p, float time, float layer, float baseHeight) {
    float depth = layer * 0.26;
    float warpStrength = 0.22 + iWarpStrength * 0.012;
    float motion = 0.28 + iSpeed * 0.06;
    float banding = clamp(iBanding * 0.01, 0.0, 1.0);

    // Per-layer personality
    float layerPhase = layer * 2.17;
    float driftDir = (layer > 1.5 && layer < 2.5) ? 1.0 : -1.0;
    float speedMul = (layer > 2.5) ? 1.6 : (0.7 + layer * 0.20);
    float layerSpeed = motion * speedMul;

    vec2 q = p;
    q.x *= 1.18 + depth * 0.32;
    q.y *= 0.70 + depth * 0.09;

    // Multi-axis motion: drift + breathe + sway
    float drift = time * layerSpeed * driftDir;
    float breathe = sin(time * 0.16 + layerPhase) * 0.07;
    float sway = sin(time * 0.38 + layerPhase * 1.3) * (0.09 + layer * 0.025);
    q += vec2(
        drift * (0.11 + depth * 0.05) + sway,
        depth * 0.15 + breathe
    );

    // Domain warping — rotational FBM for organic flowing shapes
    float warpA = fbm3(q * (0.92 + depth * 0.16) + vec2(0.0, time * 0.09 * motion));
    float warpB = vnoise(q * (1.30 + depth * 0.20) + vec2(4.1, -3.7) - vec2(time * 0.07 * motion));
    // Ridge warp adds sharp fold structures
    float warpR = ridgeNoise(q * 0.8 + vec2(time * 0.05, warpA * 2.2));

    vec2 warped = q + (vec2(warpA, warpB) - 0.5) * warpStrength;
    warped += (warpR - 0.5) * warpStrength * 0.35;

    // Curtain shape — layered sine waves at different scales
    float sweep = sin(warped.x * (2.7 + depth * 0.55) + drift * 1.08 + warpA * 5.0);
    float roll = sin(warped.x * (5.3 + depth * 0.75) - drift * 0.76 + layerPhase * 2.2);
    float ripple = sin(warped.x * (9.6 + depth * 0.4) + drift * 1.5 + warpB * 3.2) * 0.28;

    float ridge = baseHeight + layer * 0.055
        + (warpB - 0.5) * (0.53 - depth * 0.09)
        + sweep * (0.11 + depth * 0.04)
        + roll * (0.046 + depth * 0.018)
        + ripple * (0.022 + depth * 0.008);
    float drop = ridge - p.y;

    // Sharper curtain envelope — pow() drives contrast
    float curtainTop = smoothstep(-0.05, 0.18, drop);
    float curtainBot = 1.0 - smoothstep(0.72, 1.10, drop);
    float curtain = pow(curtainTop * curtainBot, 0.78 + layer * 0.08);

    // Filament structure — vertical light rays
    float folds = vnoise(vec2(warped.x * 3.7 + layerPhase * 1.6, p.y * 1.7 - drift * 0.13));
    float filaments = 0.42 + 0.58 * sin(warped.x * 12.0 + folds * 6.2 + drift * 1.35 + layerPhase);
    filaments *= 0.70 + 0.30 * sin(warped.x * 24.0 - drift * 0.68 + layerPhase * 3.0);
    // Ridge detail sharpens bright features into creases
    float ridgeDetail = ridgeNoise(warped * vec2(8.0, 2.8) + vec2(drift * 0.28, 0.0));
    filaments = filaments * 0.72 + ridgeDetail * 0.28;

    // Vertical ray fade — light drapes downward from the curtain top
    float rayFade = exp(-max(drop - 0.14, 0.0) * 3.0);
    float beam = curtain * mix(0.30, 1.0, filaments * rayFade);

    // Traveling surge — bright pulses racing along the aurora
    float surgeWave = sin(warped.x * 1.5 - time * 2.2 * motion + layerPhase * 1.4);
    float surge = pow(max(surgeWave, 0.0), 6.0) * 0.30;
    beam += surge * curtain * 0.5;

    // Zone masks for color layering
    float body = smoothstep(0.11, 0.82, drop) * (1.0 - smoothstep(0.82, 1.24, drop));
    float ribbon = smoothstep(0.04, 0.38, drop) * (1.0 - smoothstep(0.38, 0.74, drop));
    float crown = smoothstep(-0.03, 0.23, drop) * (1.0 - smoothstep(0.23, 0.49, drop));

    // Banding
    float bands = 0.5 + 0.5 * sin(drop * mix(7.0, 24.0, banding) + warped.x * 2.2 + layerPhase - drift * 0.34);
    float bandMask = mix(1.0, 0.72 + 0.28 * bands, banding * 0.85);

    // Color
    float paletteT = 0.14 + layer * 0.12 + warpA * 0.38 + filaments * 0.14 + roll * 0.06;
    float steps = mix(4.0, 11.0, banding);
    float quantizedT = floor(fract(paletteT) * steps) / steps;
    paletteT = mix(fract(paletteT), quantizedT, banding * 0.40);

    vec3 baseCol = richPaletteColor(paletteT, iPalette);
    vec3 accentCol = paletteColor(paletteT + 0.38, iPalette);
    vec3 rimCol = paletteColor(paletteT + 0.70, iPalette);

    // Palette-specific color overrides
    if (iPalette == 0) {
        baseCol = mix(baseCol, vec3(0.12, 0.90, 0.78), 0.55);
        accentCol = mix(accentCol, vec3(0.98, 0.18, 0.76), 0.78);
        rimCol = mix(rimCol, vec3(0.56, 0.20, 0.96), 0.72);
    }
    if (iPalette == 7) {
        baseCol = mix(baseCol, vec3(0.38, 0.86, 0.98), 0.48);
        accentCol = mix(accentCol, vec3(1.00, 0.32, 0.74), 0.66);
        rimCol = mix(rimCol, vec3(0.56, 0.28, 0.94), 0.58);
    }

    // Assemble color — deeper darks, brighter peaks for contrast
    vec3 col = baseCol * (0.08 + body * 0.56);
    col += mix(baseCol, accentCol, 0.62 + 0.22 * warpB) * ribbon * (0.20 + 0.28 * filaments);
    col += mix(accentCol, rimCol, 0.48 + 0.24 * (1.0 - filaments)) * crown * (0.18 + 0.30 * (0.5 + 0.5 * roll));

    if (iPalette == 0) {
        float magSweep = 0.5 + 0.5 * sin(warped.x * 6.8 - time * 1.22 * motion + layerPhase * 2.1);
        col = baseCol * (0.06 + body * 0.36);
        col += accentCol * (ribbon * (0.28 + 0.32 * filaments) + body * (0.08 + 0.24 * magSweep));
        col += rimCol * (crown * (0.22 + 0.28 * (1.0 - filaments)) + ribbon * (0.06 + 0.18 * (1.0 - magSweep)));
    }
    if (iPalette == 7) {
        float candySweep = 0.5 + 0.5 * sin(warped.x * 5.9 - time * 1.04 * motion + layerPhase * 1.7);
        col += accentCol * ribbon * (0.12 + 0.18 * candySweep);
        col += rimCol * crown * (0.10 + 0.16 * (1.0 - candySweep));
    }
    if (iPalette == 3) {
        vec3 greenCore = vec3(0.04, 0.94, 0.40);
        vec3 emerald = vec3(0.10, 0.76, 0.34);
        vec3 magenta = vec3(0.86, 0.22, 0.78);
        vec3 violet = vec3(0.44, 0.20, 0.92);

        vec3 physical = emerald * (0.14 + body * 0.66);
        physical += greenCore * (0.08 + body * 0.38 * (0.7 + 0.3 * filaments));
        physical += magenta * ribbon * (0.16 + 0.30 * filaments);
        physical += violet * crown * (0.16 + 0.32 * (1.0 - filaments * 0.4));
        col = mix(col, physical, 0.92);
    }

    // Surge accent color at bright peaks
    vec3 surgeCol = mix(accentCol, rimCol, 0.6) * 1.4;
    col += surgeCol * surge * curtain * 0.25;

    // Edge highlight — bright rim at curtain top
    float edge = smoothstep(0.02, 0.28, drop) * (1.0 - smoothstep(0.28, 0.52, drop));
    vec3 highlight = (iPalette == 3)
        ? vec3(0.16, 0.92, 0.54)
        : mix(accentCol, rimCol, 0.55 + 0.25 * filaments);
    col += highlight * edge * (0.12 + 0.14 * filaments);

    // Soft glow halo
    float glow = exp(-abs(drop - 0.16) * 5.4) * 0.26;

    float strength = beam * bandMask * (0.14 + iIntensity * 0.0075) * (1.0 - depth * 0.13);
    strength += glow * 0.05;

    return col * strength;
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    vec2 p = (uv - 0.5) * vec2(iResolution.x / iResolution.y, 1.0);
    float time = iTime * (0.58 + iSpeed * 0.30);

    // Deeper sky for stronger contrast against the aurora
    vec3 skyZenith = vec3(0.003, 0.008, 0.022);
    vec3 skyHorizon = (iPalette == 3)
        ? vec3(0.008, 0.036, 0.024)
        : richPaletteColor(0.10 + time * 0.02, iPalette) * 0.07;
    float skyMix = smoothstep(-0.48, 0.76, p.y);
    vec3 col = mix(skyHorizon, skyZenith, skyMix);

    float lowMist = 1.0 - smoothstep(-0.42, -0.02, p.y);
    col += skyHorizon * lowMist * 0.12;

    float stars = starField(uv, iTime) * (iStarBrightness * 0.010);
    col += vec3(0.48, 0.66, 0.86) * stars;

    float baseHeight = mix(-0.04, 0.28, clamp(iCurtainHeight * 0.01, 0.0, 1.0));

    // 4 aurora layers with distinct personalities
    vec3 aurora = vec3(0.0);
    for (int i = 0; i < 4; i++) {
        aurora += auroraLayer(p, time, float(i), baseHeight);
    }

    // Horizon glow
    float horizonGlow = exp(-abs(p.y + 0.18) * 6.2);
    aurora += richPaletteColor(0.24 + time * 0.03, iPalette) * horizonGlow * 0.04 * (0.35 + iIntensity * 0.010);

    col += aurora;

    if (iPalette == 3) {
        float upperTint = smoothstep(-0.04, 0.92, p.y);
        col += vec3(0.03, 0.08, 0.17) * upperTint * 0.06;
        col += vec3(0.02, 0.10, 0.06) * upperTint * 0.04;
    }

    // Ground reflection
    float groundGlow = 1.0 - smoothstep(-0.54, -0.10, p.y);
    col += aurora * groundGlow * 0.10;

    // Tone mapping
    col = max(col, vec3(0.0));
    col = 1.0 - exp(-col * (1.0 + iIntensity * 0.0024));

    float luminance = dot(col, vec3(0.2126, 0.7152, 0.0722));
    float saturation = clamp(iSaturation * 0.01, 0.0, 1.8);
    float contrast = clamp(iContrast * 0.01, 0.4, 1.8);
    col = mix(vec3(luminance), col, saturation);
    col = (col - 0.5) * contrast + 0.5;

    float peak = max(max(col.r, col.g), col.b);
    col *= 1.0 - smoothstep(0.72, 1.08, peak) * 0.14;
    col = pow(clamp(col, 0.0, 1.0), vec3(1.02));

    fragColor = vec4(col, 1.0);
}
