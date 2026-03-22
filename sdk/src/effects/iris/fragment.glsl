#version 300 es
// Iris — Geometric Audio Visualizer
// Mobius circle inversions, spiral dots, and geometric wave patterns
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

// Control uniforms (visual — used directly in shader)
uniform float iScale;
uniform float iGlowIntensity;
uniform float iIrisStrength;
uniform int iColorScheme;
uniform float iCorePulse;
uniform float iFlowDrive;
uniform float iColorAccent;
uniform float iBandSharpness;
uniform float iParticleDensity;

// Control uniforms (animation — consumed by JS frame hook, not used in shader)
uniform float iTimeSpeed;
uniform float iRotationSpeed;
uniform float iWanderSpeed;
uniform float iBeatFlash;

// Audio uniforms (auto-provided by SDK)
uniform float iAudioLevel;
uniform float iAudioBass;
uniform float iAudioMid;
uniform float iAudioTreble;
uniform float iAudioBeat;
uniform float iAudioBeatPulse;
uniform float iAudioMomentum;
uniform float iAudioSwell;
uniform float iAudioHarmonicHue;
uniform float iAudioChordMood;
uniform float iAudioOnsetPulse;
uniform float iAudioBrightness;
uniform float iAudioSpectralFlux;
uniform float iAudioTempo;
uniform vec3 iAudioFluxBands;

// State uniforms (managed in TypeScript frame hook)
uniform vec2 iSmoothMouse;
uniform float iAudioTime;
uniform float iBeatRotation;
uniform float iBeatZoom;
uniform float iRadialFlow;
uniform float iFlowVelocity;
uniform float iGlowEnergy;
uniform float iCoreEnergy;
uniform float iIrisEnergy;
uniform vec2 iSubBassDisplace;
uniform float iBeatAnticipation;
uniform float iBeatFlashOnset;

#define PI radians(180.0)
#define TAU (PI * 2.0)
#define CS(a) vec2(cos(a), sin(a))
#define PT(u, r) smoothstep(0.0, r, r - length(u))

float saturate(float x) {
    return clamp(x, 0.0, 1.0);
}

float maxChannel(vec3 color) {
    return max(color.r, max(color.g, color.b));
}

float minChannel(vec3 color) {
    return min(color.r, min(color.g, color.b));
}

vec3 hsv2rgb(vec3 c) {
    vec4 K = vec4(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    vec3 p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
    return c.z * mix(K.xxx, clamp(p - K.xxx, 0.0, 1.0), c.y);
}

vec3 acesToneMap(vec3 x) {
    x = max(vec3(0.0), x);
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
}

vec3 preserveSaturation(vec3 color, float strength) {
    float luma = dot(color, vec3(0.2126, 0.7152, 0.0722));
    vec3 delta = color - vec3(luma);
    float amt = clamp(strength, 0.0, 1.0);
    return clamp(vec3(luma) + delta * (1.0 + amt * 0.65), 0.0, 1.4);
}

// Keep bright colors out of the neutral "all channels high" zone so LEDs stay vivid.
vec3 limitWhitenessRatio(vec3 color, float maxRatio, float engageAt) {
    float peak = maxChannel(color);
    if (peak <= 0.00001) {
        return color;
    }

    float floor = minChannel(color);
    float ratio = floor / peak;
    float engage = smoothstep(engageAt, 1.0, peak) * smoothstep(maxRatio, 1.0, ratio);
    if (engage <= 0.0) {
        return color;
    }

    float targetFloor = peak * mix(ratio, maxRatio, engage);
    float spread = peak - floor;
    if (spread <= 0.00001) {
        return color * mix(1.0, 0.82, engage);
    }

    float remap = (peak - targetFloor) / spread;
    return max((color - vec3(floor)) * remap + vec3(targetFloor), 0.0);
}

float blendDetail(float derivative) {
    return smoothstep(0.0, 0.08, derivative);
}

// ---------------------------------------------------------------
// Color Palettes
// ---------------------------------------------------------------

vec3 palette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(TAU * (c * t + d));
}

vec3 getGoldBlue(float g, float l, float t) {
    vec3 col = palette(g,
        vec3(0.35, 0.35, 0.35),
        vec3(0.45, 0.4, 0.35),
        vec3(1.0, 0.9, 0.8),
        vec3(0.0, 0.1, 0.2)
    );
    col += vec3(0.9, 0.4, 0.0) * pow(g, 2.0) * 0.3;
    col += vec3(0.1, 0.4, 0.9) * pow(1.0 - g, 2.0) * 0.3;
    return col * (0.45 + l * 0.35);
}

vec3 getCyberpunk(float g, float l, float t) {
    vec3 magenta = vec3(0.9, 0.15, 0.7);
    vec3 cyan = vec3(0.1, 0.7, 0.95);
    vec3 violet = vec3(0.5, 0.2, 0.75);

    float sweep = smoothstep(0.1, 0.9, g);
    vec3 col = mix(magenta, cyan, sweep);
    col = mix(violet, col, 0.55 + l * 0.25);

    float glitch = sin(t * 0.6 + g * 7.0) * 0.5 + 0.5;
    col += vec3(0.05, 0.0, 0.1) * glitch;

    float greenPulse = clamp(iAudioBass * 0.25 + iBeatFlashOnset * 0.2, 0.0, 0.35);
    col += vec3(0.1, 0.75, 0.15) * greenPulse;

    float yellowPulse = clamp(iAudioTreble * 0.3 + iAudioMomentum * 0.2, 0.0, 0.4);
    col += vec3(0.95, 0.8, 0.1) * yellowPulse;

    return col * (0.55 + l * 0.3);
}

vec3 getAurora(float g, float l, float t) {
    vec3 col = palette(g,
        vec3(0.2, 0.35, 0.3),
        vec3(0.35, 0.45, 0.45),
        vec3(1.1, 0.8, 0.9),
        vec3(0.1, 0.4, 0.5)
    );
    col += vec3(0.05, 0.8, 0.3) * pow(g, 2.0) * 0.3;
    col += vec3(0.5, 0.15, 0.8) * pow(1.0 - g, 2.0) * 0.3;
    col += vec3(0.0, 0.5, 0.6) * sin(g * PI * 2.0) * 0.2;
    float shimmer = sin(t * 1.5 + g * 6.0) * 0.08 + 0.08;
    col += vec3(1.0, 0.5, 0.8) * shimmer * g;
    return col * (0.4 + l * 0.35);
}

vec3 getLava(float g, float l, float t) {
    vec3 col = palette(g,
        vec3(0.35, 0.2, 0.15),
        vec3(0.45, 0.35, 0.25),
        vec3(0.9, 0.6, 0.35),
        vec3(0.0, 0.05, 0.1)
    );
    col += vec3(0.35, 0.0, 0.0) * (1.0 - g) * 0.35;
    col += vec3(0.9, 0.3, 0.0) * pow(g, 1.5) * 0.4;
    col += vec3(0.9, 0.6, 0.2) * pow(g, 3.0) * 0.45;
    return col * (0.4 + l * 0.4);
}

vec3 getIce(float g, float l, float t) {
    vec3 col = palette(g,
        vec3(0.3, 0.35, 0.45),
        vec3(0.3, 0.3, 0.35),
        vec3(0.9, 0.9, 0.7),
        vec3(0.3, 0.4, 0.5)
    );
    col += vec3(0.05, 0.1, 0.5) * pow(1.0 - g, 2.0) * 0.3;
    col += vec3(0.2, 0.6, 0.9) * sin(g * PI) * 0.3;
    col += vec3(0.8, 0.9, 1.0) * pow(g, 2.0) * 0.35;
    return col * (0.45 + l * 0.35);
}

vec3 getSynesthesia(float g, float l, float t) {
    float bass = iAudioBass;
    float mid = iAudioMid;
    float treble = iAudioTreble;

    vec3 col = palette(g + bass * 0.2,
        vec3(0.35, 0.25, 0.25),
        vec3(0.4, 0.4, 0.45),
        vec3(1.0 + mid, 1.0, 1.0 + treble),
        vec3(bass * 0.3, 0.3, 0.5 + treble * 0.2)
    );

    col += vec3(0.7, 0.05, 0.15) * bass * pow(1.0 - g, 1.5) * 0.4;
    col += vec3(0.15, 0.7, 0.2) * mid * sin(g * PI) * 0.35;
    col += vec3(0.3, 0.15, 0.8) * treble * pow(g, 1.5) * 0.45;
    col += vec3(0.8, 0.4, 0.0) * (bass * treble) * 0.25;

    return col * (0.45 + l * 0.35);
}

vec3 getPhosphor(float g, float l, float t) {
    vec3 green = vec3(0.15, 0.8, 0.2);
    vec3 blue = vec3(0.05, 0.2, 0.8);
    vec3 magenta = vec3(0.8, 0.1, 0.6);
    vec3 col = mix(green, blue, pow(1.0 - g, 2.0));
    col += magenta * clamp(iAudioTreble * 0.25 + iBeatFlashOnset * 0.15, 0.0, 0.3);
    col += blue * clamp(iAudioMid * 0.15, 0.0, 0.2);
    float scan = sin(g * PI * 4.0 + t) * 0.1 + 0.85;
    col *= scan;
    return mix(vec3(0.02, 0.03, 0.05), col, 0.7 + l * 0.2);
}

vec3 getVaporwave(float g, float l, float t) {
    vec3 purple = vec3(0.4, 0.1, 0.4);
    vec3 cyan = vec3(0.0, 0.4, 0.8);
    vec3 sunset = vec3(0.9, 0.3, 0.3);
    vec3 col = mix(purple, cyan, smoothstep(0.2, 0.8, g));
    col = mix(col, sunset, clamp(iAudioBass * 0.25, 0.0, 0.3));
    col += vec3(0.15, 0.05, 0.2) * sin(t * 0.3 + g * 3.0);
    return mix(vec3(0.03, 0.02, 0.05), col, 0.65 + l * 0.25);
}

vec3 getNeonFlux(float g, float l, float t) {
    vec3 magenta = vec3(0.92, 0.08, 0.75);
    vec3 teal = vec3(0.0, 0.75, 0.75);
    vec3 amber = vec3(0.9, 0.45, 0.1);

    float bassMix = clamp(iAudioBass * 1.2, 0.0, 1.0);
    float trebleMix = clamp(iAudioTreble * 1.1, 0.0, 1.0);
    float sweep = smoothstep(0.05, 0.95, fract(g + iAudioMomentum * 0.2));

    vec3 base = mix(magenta, teal, sweep);
    vec3 accent = mix(amber, teal, bassMix);
    vec3 color = mix(base, accent, 0.35 + bassMix * 0.25);
    color += magenta * trebleMix * 0.2;
    color = clamp(color, 0.0, 0.95);
    return mix(vec3(0.05, 0.02, 0.08), color, 0.8 + l * 0.15);
}

vec3 getMidnightFlux(float g, float l, float t) {
    vec3 violet = vec3(0.6, 0.1, 0.8);
    vec3 blue = vec3(0.0, 0.25, 0.85);
    vec3 emerald = vec3(0.0, 0.5, 0.35);

    float flow = sin(t * 0.4 + g * 3.0) * 0.5 + 0.5;
    vec3 base = mix(violet, blue, flow);
    base = mix(base, emerald, clamp(iAudioLevel * 1.2, 0.0, 0.5));

    float pulse = clamp(iAudioMid * 0.7 + iAudioTreble * 0.4, 0.0, 0.8);
    base += vec3(0.2, 0.05, 0.3) * pulse;
    return base * (0.35 + l * 0.4);
}

vec3 getSolarStorm(float g, float l, float t) {
    vec3 ember = vec3(0.95, 0.35, 0.05);
    vec3 brass = vec3(0.7, 0.45, 0.1);
    vec3 teal = vec3(0.0, 0.45, 0.55);

    float sweep = fract(g + t * 0.05 + iAudioMomentum * 0.1);
    vec3 base = mix(ember, brass, smoothstep(0.2, 0.8, sweep));
    base = mix(base, teal, clamp(iAudioBass * 0.8, 0.0, 0.5));

    float flicker = 0.2 + 0.4 * sin(t * 1.2 + g * 6.0);
    base += vec3(0.1, 0.05, 0.0) * flicker;
    return base * (0.4 + l * 0.45);
}

vec3 getHarmonic(float g, float l, float t) {
    // Base hue from chromagram analysis (circle of fifths mapped)
    float baseHue = iAudioHarmonicHue;

    // Shift hue across the geometry for variation
    float roughness = iAudioSpectralFlux * 0.7;
    float hueSpread = 0.13 + roughness * 0.08;
    float hue = fract(baseHue + g * hueSpread);

    // LED-safe harmonic mapping: stay in HSV with high saturation and restrained value.
    float sat = 0.82 + iBeatFlashOnset * 0.1 + iAudioFluxBands.y * 0.08;
    sat *= 0.92 + iAudioChordMood * 0.08;
    sat = clamp(sat, 0.78, 1.0);

    float val = 0.34 + l * 0.16 + iAudioBrightness * 0.09;
    val += iBeatFlashOnset * 0.08;
    val = clamp(val, 0.28, 0.72);

    vec3 col = hsv2rgb(vec3(hue, sat, val));

    // Warm/cool temperature shift based on chord mood
    vec3 warm = vec3(1.06, 0.96, 0.88);
    vec3 cool = vec3(0.86, 0.95, 1.08);
    vec3 temperature = mix(cool, warm, iAudioChordMood * 0.5 + 0.5);
    col *= temperature;

    // Add complementary accent on treble transients
    float complement = fract(hue + 0.5);
    vec3 accentCol = hsv2rgb(vec3(complement, 0.95, 0.72));
    col += accentCol * iAudioFluxBands.z * 0.14;

    return limitWhitenessRatio(col, 0.26, 0.42);
}

vec3 getSchemeColor(float g, float l, float t) {
    // Harmonic mode: blend between base palette and chromagram-derived colors
    if (iColorScheme == 3) {
        vec3 base = getGoldBlue(g, l, t);
        vec3 harmonic = getHarmonic(g, l, t);
        return mix(base, harmonic, 0.92);
    }
    if (iColorScheme == 2) return getGoldBlue(g, l, t);
    if (iColorScheme == 1) return getCyberpunk(g, l, t);
    if (iColorScheme == 0) return getAurora(g, l, t);
    if (iColorScheme == 5) return getLava(g, l, t);
    if (iColorScheme == 4) return getIce(g, l, t);
    if (iColorScheme == 10) return getSynesthesia(g, l, t);
    if (iColorScheme == 8) return getPhosphor(g, l, t);
    if (iColorScheme == 11) return getVaporwave(g, l, t);
    if (iColorScheme == 7) return getNeonFlux(g, l, t);
    if (iColorScheme == 6) return getMidnightFlux(g, l, t);
    return getSolarStorm(g, l, t);
}

// ---------------------------------------------------------------
// Core Functions
// ---------------------------------------------------------------

vec3 gm(vec3 c, float n, float t, float w, float d, bool i) {
    float g = min(abs(n), 1.0 / abs(n));
    float s = abs(sin(n * PI - t));
    if (i) s = min(s, abs(sin(PI / n + t)));
    return (1.0 - pow(abs(s), w)) * c * pow(g, d) * 6.0;
}

float ds(vec2 u, float e, float n, float w, float h, float ro) {
    float ur = length(u);
    float sr = pow(ur, e);
    float a = round(sr) * n * TAU;
    vec2 xy = CS(a + ro) * ur;
    float l = PT(u - xy, w);
    float s = mod(sr + 0.5, 1.0);
    s = min(s, 1.0 - s);
    return l * s * h;
}

mat2 rot2(float a) {
    float c = cos(a), s = sin(a);
    return mat2(c, -s, s, c);
}

// ---------------------------------------------------------------
// Main
// ---------------------------------------------------------------

void mainImage(out vec4 outColor, vec2 fragCoord) {
    vec2 R = iResolution.xy;
    float t = iAudioTime;

    vec2 m = iSmoothMouse;
    t += m.y * iScale;

    float baseY = clamp(1.0 - abs(m.y), 0.05, 1.0);
    float baseX = clamp(1.0 - abs(m.x), 0.05, 1.0);
    float ySign = sign(m.y);
    if (ySign == 0.0) ySign = 1.0;
    float z = pow(baseY, ySign);
    float e = pow(baseX, -sign(m.x));
    float se = e * -ySign;

    vec2 uv = (fragCoord - 0.5 * R) / R.y * iScale * z;
    uv /= iBeatZoom;

    // Sub-bass displacement - whole screen moves on deep bass hits
    uv += iSubBassDisplace;

    // Beat anticipation: subtle "suck in" before the beat hits
    float anticipationScale = 1.0 + iBeatAnticipation * 0.08;
    uv *= anticipationScale;

    uv = exp(log(abs(uv) + 0.0001) * e) * sign(uv);

    // Rotation controlled entirely by TypeScript
    float totalRotation = iBeatRotation;
    uv = rot2(totalRotation) * uv;

    float px = max(length(fwidth(uv)), 0.0007);

    // Spiral flow - combines radial motion with rotation
    float rawL = length(uv);
    float angle = atan(uv.y, uv.x);

    // Spiral twist driven by flow velocity
    float spiralTwist = iRadialFlow * 0.25 / (rawL + 0.4);
    float flowAngle = angle + spiralTwist;

    // Radial warp varies with angle - breaks circular symmetry
    float radialWarp = 1.0 + sin(angle * 3.0 + iRadialFlow * 0.5) * 0.06 * iFlowVelocity;
    vec2 flowedUV = vec2(cos(flowAngle), sin(flowAngle)) * rawL * radialWarp;

    // Blend smoothly based on flow intensity
    float flowBlend = clamp(iFlowVelocity * 0.7, 0.0, 0.5);
    uv = mix(uv, flowedUV, flowBlend);

    // Additional folding based on audio
    float beatMix = pow(clamp(iBeatFlash * 0.01, 0.0, 1.0), 0.8);
    float foldStrength = 0.12 + iAudioMomentum * 0.18 + iAudioOnsetPulse * (0.04 + beatMix * 0.4);
    mat2 foldMat = mat2(1.0, foldStrength, foldStrength * -0.6, 1.0);
    vec2 foldUV = foldMat * uv;
    float x = foldUV.x;
    float y = foldUV.y;
    float l = length(uv);

    float ySafe = y;
    if (abs(ySafe) < 0.06) {
        ySafe = (ySafe >= 0.0 ? 1.0 : -1.0) * 0.06;
    }

    // Standard Mobius inversion
    float mc = (x * x + y * y - 1.0) / ySafe;
    float safeMc = max(abs(mc), 0.0001);
    float g = min(abs(mc), 1.0 / safeMc);

    // Subtle flow offset on the bands
    float bandFlow = iRadialFlow * 0.3 + sin(angle * 2.0 + iRadialFlow) * 0.1;
    float gFlowed = fract(g + bandFlow);

    float derivative = max(max(fwidth(uv.x), fwidth(uv.y)), fwidth(mc));

    // Band energies from SDK audio uniforms
    float bassEnergy = iAudioBass;
    float midEnergy = iAudioMid;
    float trebleEnergy = iAudioTreble;
    float energyMix = clamp(bassEnergy * 0.5 + midEnergy * 0.35 + trebleEnergy * 0.25, 0.0, 1.4);

    // Blend spectral flux for sharper transient response
    float fluxEnergy = iAudioFluxBands.x * 0.5 + iAudioFluxBands.y * 0.3 + iAudioFluxBands.z * 0.2;

    float irisStrength = clamp(iIrisStrength, 0.2, 4.0);
    float corePulse = clamp(iCorePulse, 0.1, 3.0);
    float flowDrive = clamp(iFlowDrive, 0.0, 4.5);
    float beatFlash = clamp(iBeatFlashOnset, 0.0, 1.6);

    // Use onset pulse for crisp beat response
    float beatPush = (0.05 + iAudioOnsetPulse * (0.15 + beatMix * 0.95) + iAudioBass * 0.22) * (0.35 + flowDrive);
    float accent = clamp(iColorAccent, 0.45, 2.4);
    float accentNorm = saturate((accent - 0.45) / 1.95);

    float paletteShift = iAudioOnsetPulse * (0.02 + beatMix * 0.12) + iAudioMomentum * 0.08 + iAudioHarmonicHue * 0.02;
    // Use gFlowed for streaming color bands
    vec3 rgb = getSchemeColor(fract(gFlowed + paletteShift), l * (1.0 + bassEnergy * 0.3), t + iAudioMid * 0.4);
    rgb = preserveSaturation(rgb, 0.2 + accentNorm * 0.28 + fluxEnergy * 0.18);
    rgb = limitWhitenessRatio(rgb, mix(0.36, 0.26, accentNorm), 0.42);

    // Audio boost with onset pulse for punch and spectral brightness for shimmer
    float audioBoost = 0.65 + iAudioLevel * 0.75 + beatFlash * 0.6 + energyMix * 0.35 + iAudioBrightness * 0.2;
    audioBoost = min(audioBoost, 2.05);
    rgb *= audioBoost;
    rgb = limitWhitenessRatio(rgb, mix(0.4, 0.3, accentNorm), 0.78);

    float bandSharp = clamp(iBandSharpness, 0.25, 3.6);
    float w = (0.025 + iAudioOnsetPulse * (0.02 + beatMix * 0.14) + iAudioFluxBands.x * 0.08) * bandSharp;
    float d = 0.18 + iAudioSwell * 0.35 + iAudioFluxBands.y * 0.35 + flowDrive * 0.06;

    vec3 c = vec3(0.0);
    c = max(c, gm(rgb, mc, -t, w, d, false));
    c = max(c, gm(rgb, (x * x) + (y * y), t, w, d, true));

    // Ribbon responds to treble flux
    float ribbon = sin(atan(y, x) * 2.0 + t * (1.0 + trebleEnergy * 0.5));
    c += rgb * ribbon * 0.08 * (trebleEnergy + iAudioFluxBands.z * 0.4);

    // Dots pulse with bass flux
    float dotBeat = 0.8 + iBeatZoom * 0.6 + iBeatFlashOnset * 0.6;
    float dotGuard = clamp(0.35 / (px + 1e-3), 0.8, 2.4);
    float dotSize = px * (1.5 + iAudioFluxBands.x * 3.0) * dotBeat * dotGuard;
    c += rgb * ds(uv, se, t / TAU, dotSize, 2.2, 0.0) * (0.7 + iAudioFluxBands.x);
    c += rgb * ds(uv, -se, t / TAU, dotSize, 2.2, PI) * (0.7 + iAudioFluxBands.z);

    // Particle system with spectral-flux-driven turbulence
    float texture = clamp(iParticleDensity, 0.0, 1.25);
    vec2 particleUV = uv * (1.0 + flowDrive * 0.45 + texture * 0.55);
    float particleAngle = atan(particleUV.y, particleUV.x);
    float particleRadius = length(particleUV);
    float streakPhase = fract(particleRadius - t * 0.5);
    float roughness = iAudioSpectralFlux * 0.7;
    float turbulence = 7.0 + roughness * 5.0 + texture * 5.5;
    float particleNoise = sin(particleAngle * turbulence + t * 1.2 + particleRadius * 6.0);
    float streakWidth = mix(0.42, 0.04, texture);
    float particleMask = smoothstep(streakWidth, 0.0, abs(fract(particleNoise) - 0.5));
    particleMask *= smoothstep(0.0, 0.8, 1.0 - particleRadius);
    particleMask *= mix(0.25, 1.1, texture);
    particleMask *= smoothstep(0.0, 1.0, streakPhase * (0.4 + texture * 0.9));
    float hueShift = 0.12 + texture * 1.15;
    vec3 particleColor = mix(
        rgb,
        getSchemeColor(fract(g * (0.2 + hueShift) - particleNoise * 0.06), l * (0.25 + texture * 0.2), t + hueShift * 1.6),
        0.15 + texture * 0.8
    );
    float particleEnergy = (0.04 + beatFlash * 0.55 + iAudioFluxBands.z * 0.4) * (0.15 + texture * 1.35);
    float particleSize = mix(0.45, 2.7, texture);
    particleMask *= particleSize;
    c += particleColor * particleMask * particleEnergy;

    // Outward beat rings triggered by onset detection
    float tempo = max(iAudioTempo / 60.0, 0.5);
    float ringPhase = fract(t * tempo * 0.1 + iAudioBeat * 0.5);
    float ringRadius = ringPhase * (1.2 + flowDrive * 0.4);
    float ringWidth = 0.018 + iAudioOnsetPulse * (0.015 + beatMix * 0.08);
    float ring = smoothstep(ringWidth, 0.0, abs(l - ringRadius));
    vec3 ringColor = mix(rgb, getSchemeColor(fract(g * 0.5 + ringPhase), l, t), 0.4);
    c += ringColor * ring * (0.08 + beatFlash * 0.7);

    // Core pulse with asymmetric smoothing
    float coreWidth = 0.08 + corePulse * 0.07;
    float core = exp(-pow(uv.x * corePulse, 2.0) / max(coreWidth, 0.05)) * exp(-l * 0.8);
    float coreBeat = iCoreEnergy;
    vec2 flowUv = uv * mat2(0.8, -0.6, 0.6, 0.8) + vec2(flowDrive * t * 0.2, t * (0.15 + beatPush * 0.05 + tempo * 0.02));
    float vascular = sin(flowUv.x * (8.0 + corePulse * 2.0) + t * (0.9 + iAudioFluxBands.x)) *
        sin(flowUv.y * (6.0 + iAudioFluxBands.y * 5.0) - t * (0.4 + flowDrive * 0.2));
    float vascularMask = smoothstep(-0.3, 0.6, vascular);
    vec3 coreColor = getSchemeColor(fract(0.5 + paletteShift * 0.5 + vascular * 0.05), l * 0.2, t * 0.7);
    vec3 coreTexture = mix(coreColor, rgb, 0.3);
    c += coreTexture * (core * 0.85 + vascularMask * (0.12 + corePulse * 0.06)) * coreBeat * (0.1 + corePulse * 0.22);

    // Iris ripples driven by mid-frequency flux
    float irisFrequency = 5.5 + irisStrength * 7.5 + iAudioFluxBands.x * 5.0;
    float irisFlowOffset = iRadialFlow * 0.8 + flowDrive * 0.6 + iBeatZoom * 0.2;
    float irisTemporal = 0.35 + flowDrive * 0.6 + iAudioFluxBands.y * 0.8 + iAudioOnsetPulse * (0.04 + beatMix * 0.9);
    float irisAngleWarp = sin(angle * 3.0 + iRadialFlow * 0.4) * 0.15;
    float irisWave = sin((l + irisFlowOffset + irisAngleWarp + beatPush) * irisFrequency - t * irisTemporal - iBeatRotation * 0.35);
    float irisMask = smoothstep(0.15, 0.96, abs(irisWave));
    float irisFeather = exp(-abs(irisWave) * (2.0 + irisStrength));
    c += rgb * irisMask * (0.08 + irisStrength * 0.05) * iIrisEnergy;
    c += rgb * irisFeather * (0.05 + irisStrength * 0.035) * iIrisEnergy;

    c = max(c, 0.0);

    float detailFactor = blendDetail(derivative);
    c = mix(rgb * (0.55 + energyMix * 0.2), c, detailFactor);
    float lowStructure = clamp(1.0 - detailFactor, 0.0, 1.0);
    vec3 fallback = getSchemeColor(fract(uv.x * 0.08 + uv.y * 0.12 + paletteShift * 0.2 + t * 0.02), l * 0.3 + 0.15, t * 0.2);
    c = mix(c, fallback, lowStructure * 0.25);

    // Chromatic aberration accent
    float aberration = (0.002 + flowDrive * 0.001) + iAudioFluxBands.z * 0.002 + iAudioBrightness * 0.001;
    vec3 fringeR = getSchemeColor(fract(g + aberration), l, t);
    vec3 fringeB = getSchemeColor(fract(g - aberration), l, t);
    vec3 aberrated = vec3(fringeR.r, c.g, fringeB.b);
    c = mix(c, aberrated, 0.25 + iAudioFluxBands.z * 0.3);

    float contrast = mix(0.85, 2.45, accentNorm);
    float pivot = dot(c, vec3(0.2126, 0.7152, 0.0722));
    c = max((c - vec3(pivot)) * contrast + vec3(pivot), 0.0);
    c = mix(c, normalize(c + 1e-4) * accent * 0.82 + c * 0.18, 0.5 + accentNorm * 0.25);

    vec2 fabricUv = uv * (0.65 + iScale * 0.2);
    float weaveA = sin(dot(fabricUv, vec2(1.7, -1.2)) + t * (0.4 + midEnergy * 0.6));
    float weaveB = sin(dot(fabricUv, vec2(-2.3, 1.15)) - t * (0.35 + trebleEnergy * 0.5));
    float weave = (weaveA + weaveB) * 0.5;
    float weaveAA = max(fwidth(weaveA), fwidth(weaveB));
    float weaveMask = smoothstep(-0.25 - weaveAA * 3.0, 0.25 + weaveAA * 3.0, weave);
    vec3 weaveColor = getSchemeColor(fract(g * 0.35 + weaveMask * 0.25 + paletteShift * 0.4), l * 0.5 + 0.2, t * 0.5);
    float weaveMix = texture * (0.1 + energyMix * 0.4);
    c = mix(c, weaveColor * (0.6 + energyMix * 0.6), weaveMix * weaveMask);

    float microSheen = sin(dot(uv, vec2(3.2, -2.7)) - t * (0.6 + trebleEnergy * 0.4));
    float microAA = fwidth(microSheen);
    microSheen = smoothstep(-0.18 - microAA * 2.0, 0.18 + microAA * 2.0, microSheen) - 0.5;
    c += rgb * microSheen * (0.03 + texture * 0.12) * (0.4 + energyMix * 0.6);

    // Glow with asymmetric smoothing - fast flash, slow fade
    float glowGain = clamp(iGlowIntensity, 0.02, 1.35);
    float glowNorm = saturate(glowGain / 1.35);
    float glowFalloff = mix(9.0, 2.15, glowNorm);
    float glow = exp(-l * glowFalloff) * glowGain;
    glow *= iGlowEnergy;
    c += rgb * glow * (0.6 + glowNorm * 0.9);

    c = preserveSaturation(c, saturate(fluxEnergy + iAudioMomentum * 0.4 + accentNorm * 0.35));
    // Limit luminance and re-saturate for RGB hardware
    float luma = dot(c, vec3(0.2126, 0.7152, 0.0722));
    float clampLuma = min(luma, 0.55 + accentNorm * 0.35);
    if (luma > 0.0) {
        c *= clampLuma / luma;
    }
    c = preserveSaturation(c, 0.35 + accentNorm * 0.35 + energyMix * 0.25);
    float vignette = smoothstep(1.35, 0.2, length(uv));
    c *= mix(0.65, 1.0, vignette);
    c = acesToneMap(c);
    c = limitWhitenessRatio(c, mix(0.42, 0.3, accentNorm), 0.74);
    c = clamp(c, 0.0, 1.0);

    outColor = vec4(c, 1.0);
}

void main() {
    mainImage(fragColor, gl_FragCoord.xy);
}
