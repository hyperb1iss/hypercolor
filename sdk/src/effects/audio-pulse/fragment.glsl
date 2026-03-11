#version 300 es
// Audio Pulse — 3D Audio Visualizer
// Based on "3D Audio Visualizer" by @kishimisu (CC BY-NC-SA 4.0)
// https://www.shadertoy.com/view/dtl3Dr
precision highp float;

out vec4 fragColor;

// Standard uniforms
uniform float iTime;
uniform vec2 iResolution;

// Control uniforms (raw values from SDK controls — normalized in-shader)
uniform float iSensitivity;   // 10–200
uniform float iSmoothing;     // 0–95
uniform float iBassBoost;     // 0–200
uniform float iColorSpeed;    // 0–200
uniform int iRingCount;       // 4–16
uniform float iGlowIntensity; // 0–200
uniform float iDirection;     // -360–360
uniform float iBend;          // -200–200
uniform float iFlow;          // -100–100
uniform int iVisualStyle;     // 0–3
uniform int iColorScheme;     // 0–5

// Audio uniforms (auto-provided by SDK when audio: true)
uniform float iAudioLevel;
uniform float iAudioBass;
uniform float iAudioMid;
uniform float iAudioTreble;
uniform float iAudioBeat;
uniform float iAudioBeatPulse;
uniform float iAudioTempo;
uniform float iAudioBrightness;
uniform float iAudioMomentum;
uniform float iAudioSwell;

#define PI 3.14159265359
#define TAU 6.28318530718

// ─────────────────────────────────────────────────────────────
// Control normalization helpers
// ─────────────────────────────────────────────────────────────

float nSensitivity() { return clamp(iSensitivity / 100.0, 0.1, 2.0) * 1.5; }
float nSmoothing()   { return clamp(iSmoothing / 95.0, 0.0, 1.0); }
float nBassBoost()   { return clamp(iBassBoost / 100.0, 0.0, 2.0); }
float nColorSpeed()  { return clamp(iColorSpeed / 100.0, 0.0, 2.0); }
float nGlowFactor()  { return clamp(iGlowIntensity / 100.0 * 0.25, 0.0, 1.0); }
float nDirection()   { return iDirection / 180.0; }
float nBend()        { return clamp(iBend / 100.0, -2.0, 2.0); }
float nFlow()        { return clamp(iFlow / 100.0, -1.0, 1.0); }

// ─────────────────────────────────────────────────────────────
// Core Functions (from kishimisu)
// ─────────────────────────────────────────────────────────────

// Band-interpolated spectrum approximation (replaces texture2D sampling)
float getFreq(float index) {
    float idx = clamp(index, 0.0, 1.0);
    float bassW = exp(-idx * idx * 30.0);
    float midW = exp(-(idx - 0.35) * (idx - 0.35) * 12.0);
    float trebleW = exp(-(idx - 0.75) * (idx - 0.75) * 10.0);
    float total = bassW + midW + trebleW + 0.001;
    float level = (iAudioBass * bassW + iAudioMid * midW + iAudioTreble * trebleW) / total;
    // Per-frequency variation from hash for texture
    float hash = fract(sin(idx * 127.1 + 311.7) * 43758.5453);
    float variation = 0.7 + 0.3 * sin(iTime * 0.4 + hash * 6.28);
    return level * variation;
}

// Logistic function for smooth amplitude scaling
float logX(float x, float a, float c) {
    return 1.0 / (exp(-a * (x - c)) + 1.0);
}

// Normalize amplitude with logistic curve (prevents harsh cutoffs)
float logisticAmp(float amp) {
    float c = 0.88, a = 20.0;
    return (logX(amp, a, c) - logX(0.0, a, c)) / (logX(1.0, a, c) - logX(0.0, a, c));
}

// Smoothed spectrum sampling with neighborhood averaging
float smoothSpectrum(float idx) {
    float sum = 0.0;
    float total = 0.0;
    for (float offset = -2.0; offset <= 2.0; offset += 1.0) {
        float weight = 1.0 - abs(offset) * 0.2;
        sum += getFreq(clamp(idx + offset * 0.005, 0.0, 1.0)) * weight;
        total += weight;
    }
    return sum / total;
}

// Get pitch/frequency value with logistic scaling
float getPitch(float freq, float scale) {
    float sens = nSensitivity();
    float smoothing = nSmoothing();

    float baseRaw = smoothSpectrum(freq) * sens * scale;
    float base = logisticAmp(clamp(baseRaw, 0.0, 1.0));

    if (smoothing > 0.001) {
        float offset = 0.01 + smoothing * 0.02;
        float aheadRaw = smoothSpectrum(clamp(freq + offset, 0.0, 1.0)) * sens * scale * 0.95;
        float behindRaw = smoothSpectrum(clamp(freq - offset, 0.0, 1.0)) * sens * scale * 0.95;
        float ahead = logisticAmp(clamp(aheadRaw, 0.0, 1.0));
        float behind = logisticAmp(clamp(behindRaw, 0.0, 1.0));
        float neighborhood = (base + ahead + behind) / 3.0;
        float globalLevel = clamp(iAudioLevel * (0.5 + smoothing * 0.5), 0.0, 1.0);
        float blended = mix(neighborhood, globalLevel, 0.25 + smoothing * 0.35);
        base = mix(base, blended, smoothing * 0.7);
    }

    return base;
}

// Get volume/energy level
float getVol(float samples) {
    float sens = nSensitivity();
    float vol = 0.0;
    for (float i = 0.0; i < 8.0; i++) {
        vol += smoothSpectrum(i / samples);
    }
    return vol / 8.0 * sens;
}

vec2 hash22(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * vec3(0.1031, 0.1030, 0.0973));
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((vec2(p3.x, p3.y) + p3.z) * vec2(5.0, 5.5));
}

float hash13(vec3 p3) {
    p3 = fract(p3 * 0.1031);
    p3 += dot(p3, p3.zyx + 31.32);
    return fract((p3.x + p3.y) * p3.z);
}

float hash12(vec2 p) {
    vec2 h = hash22(p);
    return fract(h.x + h.y);
}

// Signed distance function for box
float sdBox(vec3 p, vec3 b) {
    vec3 q = abs(p) - b;
    return length(max(q, 0.0)) + min(max(q.x, max(q.y, q.z)), 0.0);
}

// Light attenuation
float light(float d, float att) {
    return 1.0 / (1.0 + pow(max(abs(d * att), 0.0001), 1.3));
}

mat2 rotate2d(float angle) {
    float c = cos(angle);
    float s = sin(angle);
    return mat2(c, -s, s, c);
}

// HSV to RGB
vec3 hsv2rgb(vec3 c) {
    vec4 K = vec4(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    vec3 p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
    return c.z * mix(K.xxx, clamp(p - K.xxx, 0.0, 1.0), c.y);
}

// ─────────────────────────────────────────────────────────────
vec3 boostSaturation(vec3 color, float boost) {
    float luma = dot(color, vec3(0.2126, 0.7152, 0.0722));
    vec3 delta = color - vec3(luma);
    return clamp(vec3(luma) + delta * (1.0 + boost), 0.0, 1.15);
}

float getGlowGain() {
    float normalized = nGlowFactor();
    float eased = pow(normalized, 0.85);
    return mix(0.08, 0.85, eased);
}

vec3 limitVibrancy(vec3 color) {
    float maxChannel = max(max(color.r, color.g), color.b);
    float minChannel = min(min(color.r, color.g), color.b);
    float saturation = maxChannel - minChannel + 1e-4;

    if (maxChannel > 0.78) {
        float target = mix(0.74, 0.9, smoothstep(0.78, 1.05, maxChannel));
        color *= target / (maxChannel + 1e-4);
    }

    float luma = dot(color, vec3(0.2126, 0.7152, 0.0722));
    float satBoost = smoothstep(0.0, 0.65, saturation);
    color = mix(vec3(luma), color, 0.6 + satBoost * 0.25);

    float saturationGuard = smoothstep(0.15, 0.0, minChannel);
    color = mix(color, mix(color, vec3(luma), 0.2), saturationGuard * 0.15);
    return clamp(color, 0.0, 1.0);
}

// Color Schemes
// 0: Cyberpunk, 1: Lava, 2: Aurora, 3: Vaporwave, 4: Toxic, 5: Prism
// ─────────────────────────────────────────────────────────────

vec3 getSchemeColor(vec3 id, float t) {
    vec3 a;
    vec3 b;

    if (iColorScheme == 1) {
        a = vec3(0.98, 0.05, 0.78);
        b = vec3(0.05, 0.75, 1.2);
    } else if (iColorScheme == 2) {
        a = vec3(1.2, 0.25, 0.05);
        b = vec3(0.9, 0.55, 0.05);
    } else if (iColorScheme == 0) {
        a = vec3(0.1, 0.8, 0.7);
        b = vec3(0.2, 0.4, 1.2);
    } else if (iColorScheme == 5) {
        a = vec3(1.1, 0.45, 0.8);
        b = vec3(0.35, 0.9, 1.2);
    } else if (iColorScheme == 4) {
        a = vec3(0.35, 1.15, 0.2);
        b = vec3(0.05, 0.65, 0.95);
    } else {
        vec3 rainbow = 0.6 + 0.6 * cos(id * 0.8 + vec3(0.0, 2.0, 4.0) + t);
        return boostSaturation(rainbow, 0.9);
    }

    float wave = 0.5 + 0.5 * sin(t + dot(id, vec3(0.7, 0.3, 0.55)));
    vec3 color = mix(a, b, wave);
    color += 0.15 * sin(vec3(0.0, 2.0, 4.0) + t * 0.8 + id.yzx * 0.6);
    return boostSaturation(color, 0.8);
}

// ─────────────────────────────────────────────────────────────
// Style 0: Pulse Field lattice (sparkling cube bloom)
// ─────────────────────────────────────────────────────────────

vec3 pulseFieldStyle(vec2 uv, float time) {
    float bassBoost = nBassBoost();
    float colorSpeed = nColorSpeed();
    float dir = nDirection() * PI;
    float bend = nBend();
    float flow = nFlow();

    vec3 col = vec3(0.0);
    float vol = clamp(getVol(8.0) * 1.05, 0.0, 1.3);
    float bassPulse = clamp(iAudioBass * bassBoost * 0.22, 0.0, 1.6);
    float midPulse = iAudioMid;
    float treblePulse = iAudioTreble;
    float glowGain = getGlowGain();
    float audioMomentum = clamp(iAudioMomentum, -1.0, 1.0);
    float swell = clamp(iAudioSwell, 0.0, 1.0);
    float baseFlow = clamp(flow + audioMomentum * 0.35, -1.5, 1.5);
    float beatOffset = iAudioBeatPulse * (baseFlow >= 0.0 ? 1.0 : -1.0) * 0.35;
    float finalFlow = clamp(baseFlow + beatOffset, -1.8, 1.8);
    float travelDir = finalFlow >= 0.0 ? -1.0 : 1.0;

    float axialShift = time * finalFlow * (1.1 + swell * 0.8);

    vec3 ro = vec3(
        sin(time * 0.22 + dir) * (1.0 + swell * 0.25),
        cos(time * 0.19 + dir * 0.85) * (0.95 + swell * 0.2),
        axialShift
    );
    ro.xy += vec2(
        sin(time * 0.3 + bend * 0.2),
        cos(time * 0.27 - bend * 0.1)
    ) * (0.35 + swell * 0.2);
    ro += vec3(0.0, 0.0, travelDir * (0.6 + bassPulse * 0.9 + swell * 0.4));

    float zoom = 1.0 + bassPulse * 0.25 + vol * 0.2 + iAudioBeatPulse * 0.25;
    vec3 rd = normalize(vec3(uv * (0.85 + bassPulse * 0.12 - audioMomentum * 0.08), zoom));
    rd.xy = rotate2d(time * 0.06 + vol * 0.08 + dir + audioMomentum * 0.2) * rd.xy;
    rd.y += sin(time * 0.15 + uv.x * 2.0) * (0.03 + swell * 0.02);
    rd.x += sin(time * 0.12 + uv.y * 1.8) * (0.03 + swell * 0.02);
    rd.z += bend * 0.035 + finalFlow * 0.05;

    float travel = 0.0;
    float glowMix = mix(0.02, 0.55, glowGain);

    for (int i = 0; i < 70; i++) {
        vec3 p = ro + rd * travel * travelDir;
        vec3 scenePos = p;
        float swirlPhase = time * 0.18 + dot(scenePos, vec3(0.05, 0.07, 0.09));
        vec3 flowField = vec3(
            sin(swirlPhase),
            cos(swirlPhase),
            sin(swirlPhase * 0.7)
        ) * (0.1 + abs(finalFlow) * 0.18 + swell * 0.08);
        scenePos += flowField;
        scenePos.xy += finalFlow * vec2(scenePos.y, -scenePos.x) * 0.1;
        scenePos.xy += bend * vec2(scenePos.y, -scenePos.x) * 0.02;
        scenePos.z += bassPulse * 0.2 + swell * 0.1;

        vec3 cell = floor(scenePos);
        vec3 local = fract(scenePos) - 0.5;

        float freqIdx = fract((cell.x * 0.31 + cell.y * 0.21 + cell.z * 0.13) * 0.25 + bassPulse * 0.02);
        float pitched = getPitch(freqIdx, 0.9 + vol * 0.3 + swell * 0.4);
        float bandMorph = smoothstep(0.0, 1.0, fract(dot(cell, vec3(0.17, 0.11, 0.07))));
        float bandEnergy = mix(bassPulse, treblePulse, bandMorph);
        float smoothEnergy = mix(midPulse, bandEnergy, 0.6);
        float blended = mix(pitched, smoothEnergy, 0.6);
        float amp = mix(blended, 0.2 + vol * 0.4 + swell * 0.3, 0.4);
        amp = smoothstep(0.0, 1.0, amp);

        float sizeBase = 0.2 + amp * 0.17 + midPulse * 0.05;
        float cube = sdBox(local, vec3(sizeBase));
        float glowDist = max(cube, 0.0);
        float sparkle = exp(-glowDist * (14.0 + treblePulse * 8.0)) / (0.5 + glowDist * 32.0);
        float swirl = sin(dot(cell, vec3(0.32, 0.52, 0.71)) + time * 0.35 + freqIdx * 2.6);
        float flicker = 0.82 + 0.18 * swirl;

        vec3 schemeCol = getSchemeColor(cell + vec3(0.0, 0.0, axialShift * 0.25), time * colorSpeed * 0.3 + freqIdx * 2.0);
        float energy = (0.035 + amp * 0.32 + vol * 0.08 + swell * 0.1) * glowMix;
        col += schemeCol * sparkle * flicker * energy;

        float warpTrail = exp(-abs(local.z) * (7.5 - amp * 3.0));
        col += schemeCol * warpTrail * 0.007 * (0.25 + amp * 0.4) * glowMix;

        float stepLen = max(abs(cube), 0.035 + amp * 0.02);
        travel += stepLen;
        if (travel > 34.0) break;
    }

    float fog = (vol * 0.012 + swell * 0.02) * glowMix;
    vec3 fogCol = getSchemeColor(vec3(2.5, 0.5, 1.0), time * 0.15 + bassPulse * 0.2);
    float centerFalloff = 1.0 - smoothstep(0.0, 1.2, length(uv + vec2(bend * 0.08, 0.0)));
    col += fogCol * fog * (0.35 + centerFalloff * 0.65);

    col = col / (1.0 + col * (0.8 + glowGain * 0.2));
    col = clamp(col, 0.0, 1.0);

    return col;
}

// ─────────────────────────────────────────────────────────────
// Style 1: Grid - Infinite flying grid of reactive cubes
// ─────────────────────────────────────────────────────────────

vec3 gridStyle(vec2 uv, float time) {
    float colorSpeed = nColorSpeed();
    float dir = nDirection() * PI;
    float bend = nBend();
    float flowRaw = nFlow();

    vec3 col = vec3(0.0);
    float vol = clamp(getVol(8.0), 0.0, 1.2);
    float glowGain = getGlowGain();
    float tempoPhase = (iAudioTempo / 180.0) * time;
    float audioMomentum = clamp(iAudioMomentum, -1.0, 1.0);
    float swell = clamp(iAudioSwell, 0.0, 1.0);
    float flow = clamp(flowRaw + audioMomentum * 0.4, -1.8, 1.8);
    float flowDir = flow >= 0.0 ? -1.0 : 1.0;
    float travelSpeed = 1.2 + abs(flow) * 1.5 + swell * 0.8;
    float beatDrive = 1.0 + iAudioBeatPulse * 0.3;

    // Approximate envelope uniforms from band levels
    float bassEnv = max(0.0, iAudioBass * 1.5 - 0.2);
    float trebleEnv = max(0.0, iAudioTreble * 1.5 - 0.2);

    vec2 warped = rotate2d(dir) * uv;
    warped.x += sin(time * 0.22 + warped.y * 2.1) * 0.1 * bend;
    warped.y += cos(time * 0.18 - warped.x * 1.7) * 0.08 * bend;

    vec3 rd = normalize(vec3(warped * (0.75 + vol * 0.2), flowDir * (1.1 + swell * 0.4)));
    rd.xy = rotate2d(time * 0.08 + dir * 0.5 + audioMomentum * 0.3) * rd.xy;
    rd.z += flowDir * (0.25 + swell * 0.4);

    vec3 ro = vec3(0.0, 0.0, time * travelSpeed * flowDir * beatDrive);
    ro.xy += vec2(sin(time * 0.25 + tempoPhase), cos(time * 0.18 - tempoPhase)) * (0.2 + vol * 0.25 + iAudioBeat * 0.2);
    ro.xy += rotate2d(dir + time * 0.05) * vec2(audioMomentum * 0.6, swell * 0.4);
    ro.xy += bend * vec2(uv.y, -uv.x) * 0.2;

    for (float i = 0.0, t = 0.0; i < 60.0; i++) {
        vec3 p = ro + t * rd;
        p.xy += bend * vec2(p.y, -p.x) * 0.05;
        p.xy += flow * vec2(p.y, -p.x) * 0.03;
        p.xy = rotate2d(sin(time * 0.04 + p.z * 0.3) * 0.2) * p.xy;

        vec3 id = floor(p);
        vec3 q = fract(p) - 0.5;

        float freqIdx = mod(abs(id.x) + abs(id.y) * 2.0 + abs(id.z) * 0.5, 32.0) / 32.0;
        float env = mix(bassEnv, trebleEnv, freqIdx);
        float amp = mix(getPitch(freqIdx, 1.0 + vol * 0.35 + swell * 0.4), env * 1.8, 0.55);

        float boxSize = 0.23 + amp * 0.18;
        float d = sdBox(q, vec3(boxSize));

        float fade = exp(-t * 0.05) * (0.7 + amp * 0.3) * (0.85 + swell * 0.3);
        vec3 schemeCol = getSchemeColor(id + vec3(0.0, 0.0, tempoPhase), time * colorSpeed * 0.32 + freqIdx * 3.0);

        float crisp = pow(max(0.0, 1.0 - max(d, 0.0) * (11.0 + amp * 10.0)), 2.2);
        float spark = exp(-max(d, 0.0) * (18.0 + amp * 26.0));
        float edge = smoothstep(0.18, 0.0, abs(d)) * (0.25 + amp * 0.4);
        float ribbon = exp(-abs(q.z) * (24.0 + env * 14.0)) * (0.1 + amp * 0.3);
        float glow = (crisp * 0.7 + spark * 0.4 + edge * 0.3 + ribbon * 0.6) * (0.4 + amp * 0.8);
        col += schemeCol * glow * fade * (0.5 + vol * 0.5) * glowGain * 0.65;

        float stepSize = max(abs(d), 0.04 + 0.03 * amp) / (beatDrive * (1.0 + abs(flow) * 0.4));
        t += stepSize;
        if (t > 35.0) break;
    }

    return col;
}

// ─────────────────────────────────────────────────────────────
// Style 2: Waveform - 3D frequency ribbons
// ─────────────────────────────────────────────────────────────

vec3 waveformStyle(vec2 uv, float time) {
    float colorSpeed = nColorSpeed();
    float dir = nDirection() * PI * 0.5;
    float bend = nBend();
    float flow = nFlow();

    // Approximate envelope uniforms from band levels
    float bassEnv = max(0.0, iAudioBass * 1.5 - 0.2);
    float trebleEnv = max(0.0, iAudioTreble * 1.5 - 0.2);

    vec3 col = vec3(0.0);
    float glowGain = getGlowGain();
    float audioMomentum = clamp(iAudioMomentum, -1.0, 1.0);
    float swell = clamp(iAudioSwell, 0.0, 1.0);

    vec2 warped = rotate2d(dir) * uv;
    warped.x += time * (flow * 0.45 + audioMomentum * 0.35);
    warped.x += sin(time * 0.23 + warped.y * 2.0) * bend * 0.12;
    warped.y += cos(time * 0.2 - warped.x * 1.6) * bend * 0.08;
    warped.y += sin(warped.x * 1.8 + time * 0.4) * swell * 0.1;

    float density = clamp(iAudioBrightness, 0.0, 1.0);
    float baseHeight = 0.22 + swell * 0.25;

    for (int layer = 0; layer < 4; layer++) {
        float fi = float(layer);
        float layerDepth = 1.0 - fi * 0.22;
        float layerTime = time * (0.7 + fi * 0.12);
        float freqIdx = fract((warped.x * 0.35 + layerTime * 0.2) + fi * 0.17);
        float amp = getPitch(freqIdx, 1.0 + swell * 0.6 + fi * 0.2);
        float env = mix(bassEnv, trebleEnv, fi / 3.0);
        float energy = mix(amp, env * 1.5, 0.5);
        float crest = sin(warped.x * (3.4 + fi * 0.7) + layerTime * 2.0 + iAudioBeatPulse * 2.5);
        float y = crest * (baseHeight + fi * 0.08 + energy * 0.3) - fi * 0.32;
        y += sin(warped.x * 1.5 + time * 0.5 + fi) * audioMomentum * 0.12;

        float band = exp(-abs(warped.y - y) * (180.0 + energy * 90.0)) * (0.3 + energy * 0.9);
        float filament = exp(-abs(warped.y - y) * (420.0 + energy * 150.0));
        float haze = exp(-abs(warped.y - y) * 24.0) * (0.04 + energy * 0.07);
        float dotPhase = fract((warped.x + layerTime * 0.8) * 6.0 + fi * 0.5);
        float sparkMask = smoothstep(0.4, 0.0, abs(dotPhase - 0.5));
        float spark = filament * sparkMask * (0.25 + density * 0.5);

        vec3 schemeCol = getSchemeColor(vec3(fi * 1.5, freqIdx * 10.0, layerTime), time * colorSpeed * 0.45 + fi * 0.2);
        col += schemeCol * (band + spark) * layerDepth * glowGain;
        col += schemeCol * haze * layerDepth * 0.4;
    }

    vec3 bgLow = getSchemeColor(vec3(-2.0, 0.0, 0.0), time * 0.1);
    vec3 bgHigh = getSchemeColor(vec3(4.0, 1.5, 0.0), time * -0.05);
    float gradient = smoothstep(-0.8, 0.9, warped.y + 0.05);
    col += mix(bgLow, bgHigh, gradient) * 0.05;

    float trail = exp(-abs(warped.x) * 1.5) * exp(-abs(warped.y) * 2.2) * swell * 0.25;
    col += getSchemeColor(vec3(0.0, 6.0, 0.0), time * 0.3) * trail;

    return col;
}

// ─────────────────────────────────────────────────────────────
// Style 3: Vortex - Spiral tunnel with frequency bands
// ─────────────────────────────────────────────────────────────

vec3 vortexStyle(vec2 uv, float time) {
    float bassBoost = nBassBoost();
    float colorSpeed = nColorSpeed();
    float dir = nDirection() * PI;
    float bend = nBend();
    float flowRaw = nFlow();

    vec3 col = vec3(0.0);
    float vol = clamp(getVol(8.0) * 1.1, 0.0, 1.6);
    float bassPulse = clamp(iAudioBass * bassBoost * 0.18, 0.0, 1.8);
    float treble = iAudioTreble;
    float glowGain = getGlowGain();
    float audioMomentum = clamp(iAudioMomentum, -1.0, 1.0);
    float swell = clamp(iAudioSwell, 0.0, 1.0);
    float flow = clamp(flowRaw + audioMomentum * 0.5, -1.5, 1.5);
    float swirlDir = flow >= 0.0 ? 1.0 : -1.0;
    float swirlSpeed = 0.4 + abs(flow) * 0.8 + swell * 0.6;
    float rings = float(iRingCount);

    vec2 rotated = rotate2d(dir) * uv;
    rotated += vec2(
        sin(time * 0.3 + rotated.y * 2.0),
        cos(time * 0.26 - rotated.x * 2.0)
    ) * bend * 0.1;

    vec2 swirlUV = rotate2d(time * swirlSpeed * swirlDir + iAudioBeatPulse * 0.15) * rotated;
    float r = length(swirlUV);
    float a = atan(swirlUV.y, swirlUV.x);
    float spacing = mix(0.09, 0.16, clamp(rings / 12.0, 0.0, 1.0));

    for (int i = 0; i < 24; i++) {
        if (i >= iRingCount) break;
        float fi = float(i);
        float freqIdx = fi / max(rings - 1.0, 1.0);
        float amp = max(0.05, getPitch(freqIdx, 1.0 + treble * 0.7 + swell * 0.6));
        float ringR = 0.12 + fi * spacing + amp * 0.12 + vol * 0.05 + bassPulse * 0.05;
        ringR += sin(time * 0.4 + fi * 0.6 + iAudioBeatPulse * 0.8) * (0.015 + swell * 0.03);
        float spiralPhase = fract(a / TAU + fi * 0.11 - time * swirlSpeed * swirlDir * 0.5);
        float spiral = sin(spiralPhase * TAU + flow * 2.0) * (0.04 + amp * 0.05);

        float d = abs(r - ringR - spiral);
        float glow = exp(-d * d * (140.0 + amp * 90.0)) * (0.4 + amp * 0.9);
        float segments = max(rings * 1.5, 6.0);
        float segmentPhase = fract(a / TAU * segments + fi * 0.3 + time * 0.35);
        glow *= 0.5 + getPitch(segmentPhase, 0.9);

        vec3 schemeCol = getSchemeColor(vec3(fi * 1.2, ringR * 7.0, 0.0), time * colorSpeed * 0.45 + freqIdx * 1.8);
        col += schemeCol * glow * glowGain;
    }

    for (int p = 0; p < 40; p++) {
        float fp = float(p) / 40.0;
        float freqIdx = fract(fp + time * 0.1);
        float amp = getPitch(freqIdx, 0.8 + iAudioBass * 0.5);
        float streakAngle = a + fp * TAU * swirlDir + time * 0.2;
        float sweep = sin(streakAngle) * 0.5 + 0.5;
        float streak = exp(-abs(fract(streakAngle / TAU) - 0.5) * 18.0) * exp(-abs(r - fp) * 8.0);
        vec3 schemeCol = getSchemeColor(vec3(freqIdx * 10.0, fp * 4.0, 0.0), time * 0.2 + fp);
        col += schemeCol * streak * sweep * amp * 0.12;
    }

    float arms = 6.0 + rings * 0.3;
    float armPattern = sin(a * arms - time * (1.1 + treble) + flow * 2.0) * 0.5 + 0.5;
    float armGlow = exp(-abs(armPattern - r * 0.8) * 5.5) * exp(-r * 1.4);
    col += getSchemeColor(vec3(arms, 0.5, 0.0), time * 0.45) * armGlow * (0.3 + treble * 0.6);

    float centerGlow = exp(-r * 3.6) * (0.5 + vol * 0.5 + iAudioBeatPulse * 0.25 + swell * 0.3);
    col += getSchemeColor(vec3(0.0, 3.0, 0.0), time * 0.9) * centerGlow * glowGain;

    float rim = smoothstep(0.2, 1.4, r);
    col *= mix(1.0, 0.78, rim * 0.4);

    return col;
}

// ─────────────────────────────────────────────────────────────
// Main
// ─────────────────────────────────────────────────────────────

void mainImage(out vec4 fragOut, vec2 fragCoord) {
    vec2 uv = (2.0 * fragCoord - iResolution.xy) / iResolution.y;
    float t = iTime;

    vec3 color = vec3(0.0);

    // Select visualization style
    if (iVisualStyle == 1) {
        color = pulseFieldStyle(uv, t);
    } else if (iVisualStyle == 0) {
        color = gridStyle(uv, t);
    } else if (iVisualStyle == 3) {
        color = waveformStyle(uv, t);
    } else {
        color = vortexStyle(uv, t);
    }

    // Global audio modulation
    float vol = getVol(8.0);
    color *= 0.85 + vol * 0.35 + iAudioSwell * 0.2;

    // Keep saturation without extra bloom/blur
    color = boostSaturation(color, 0.2);

    // Tone mapping (soft)
    color = color / (1.0 + color * 0.65);
    color = limitVibrancy(color);

    fragOut = vec4(clamp(color, 0.0, 1.0), 1.0);
}

void main() {
    mainImage(fragColor, gl_FragCoord.xy);
}
