#version 300 es
// Audio Pulse — 3D Audio Visualizer
// Based on "3D Audio Visualizer" by @kishimisu (CC BY-NC-SA 4.0)
// https://www.shadertoy.com/view/dtl3Dr
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

// Controls
uniform float iSensitivity;   // 10–200
uniform float iFlow;          // -100–100
uniform float iGlowIntensity; // 10–200
uniform float iColorSpeed;    // 0–200
uniform int iVisualStyle;     // 0–3
uniform int iColorScheme;     // 0–5

// Audio (SDK auto-provides)
uniform float iAudioLevel;
uniform float iAudioBass;
uniform float iAudioMid;
uniform float iAudioTreble;
uniform float iAudioBeatPulse;
uniform float iAudioMomentum;
uniform float iAudioSwell;

#define PI 3.14159265359
#define TAU 6.28318530718

// ─── Normalized controls ─────────────────────────────────────

float sens()  { return clamp(iSensitivity / 100.0, 0.1, 2.0); }
float flowN() { return clamp(iFlow / 100.0, -1.0, 1.0); }
float glowN() { return clamp(iGlowIntensity / 100.0, 0.1, 2.0); }
float cSpd()  { return clamp(iColorSpeed / 100.0, 0.0, 2.0); }

// ─── Audio helpers ───────────────────────────────────────────

// Interpolate bass/mid/treble into a pseudo-spectrum
float getFreq(float idx) {
    idx = clamp(idx, 0.0, 1.0);
    float bW = exp(-idx * idx * 30.0);
    float mW = exp(-(idx - 0.35) * (idx - 0.35) * 12.0);
    float tW = exp(-(idx - 0.75) * (idx - 0.75) * 10.0);
    float total = bW + mW + tW + 0.001;
    return (iAudioBass * bW + iAudioMid * mW + iAudioTreble * tW) / total;
}

float pitch(float freq, float scale) {
    return clamp(getFreq(freq) * sens() * scale, 0.0, 1.0);
}

float vol() { return clamp(iAudioLevel * sens(), 0.0, 1.2); }

// ─── Utilities ───────────────────────────────────────────────

float sdBox(vec3 p, vec3 b) {
    vec3 q = abs(p) - b;
    return length(max(q, 0.0)) + min(max(q.x, max(q.y, q.z)), 0.0);
}

mat2 rot2(float a) {
    float c = cos(a), s = sin(a);
    return mat2(c, -s, s, c);
}

vec3 satBoost(vec3 c, float b) {
    float l = dot(c, vec3(0.2126, 0.7152, 0.0722));
    return clamp(vec3(l) + (c - vec3(l)) * (1.0 + b), 0.0, 1.15);
}

// ─── Color Schemes ───────────────────────────────────────────
// Combo indices: Aurora(0), Cyberpunk(1), Lava(2), Prism(3), Toxic(4), Vaporwave(5)

vec3 scheme(vec3 id, float t) {
    vec3 a, b;
    if (iColorScheme == 1) {         // Cyberpunk
        a = vec3(0.98, 0.05, 0.78); b = vec3(0.05, 0.75, 1.2);
    } else if (iColorScheme == 2) {  // Lava
        a = vec3(1.2, 0.25, 0.05);  b = vec3(0.9, 0.55, 0.05);
    } else if (iColorScheme == 0) {  // Aurora
        a = vec3(0.1, 0.8, 0.7);    b = vec3(0.2, 0.4, 1.2);
    } else if (iColorScheme == 5) {  // Vaporwave
        a = vec3(1.1, 0.45, 0.8);   b = vec3(0.35, 0.9, 1.2);
    } else if (iColorScheme == 4) {  // Toxic
        a = vec3(0.35, 1.15, 0.2);  b = vec3(0.05, 0.65, 0.95);
    } else {                         // Prism
        return satBoost(0.6 + 0.6 * cos(id * 0.8 + vec3(0.0, 2.0, 4.0) + t), 0.9);
    }
    float wave = 0.5 + 0.5 * sin(t + dot(id, vec3(0.7, 0.3, 0.55)));
    vec3 c = mix(a, b, wave) + 0.15 * sin(vec3(0.0, 2.0, 4.0) + t * 0.8 + id.yzx * 0.6);
    return satBoost(c, 0.8);
}

// ─── Style 1: Pulse Field ────────────────────────────────────
// Ethereal lattice of glowing cubes flowing through 3D space.
// Audio drives cube size and camera movement — not brightness.

vec3 pulseFieldStyle(vec2 uv, float time) {
    float v = vol();
    float swell = clamp(iAudioSwell, 0.0, 1.0);
    float momentum = clamp(iAudioMomentum, -1.0, 1.0);
    float bass = clamp(iAudioBass * sens(), 0.0, 1.5);
    float fl = flowN();
    float cs = cSpd();
    float g = glowN();

    // Flow direction with audio momentum
    float totalFlow = clamp(fl + momentum * 0.35 + iAudioBeatPulse * sign(fl + 0.001) * 0.3, -1.8, 1.8);
    float travelDir = totalFlow >= 0.0 ? -1.0 : 1.0;

    // Camera — audio drives position and speed
    float drift = time * totalFlow * (1.3 + swell * 1.0 + v * 0.5);
    vec3 ro = vec3(
        sin(time * 0.22) * (1.0 + swell * 0.4 + v * 0.3),
        cos(time * 0.19) * (0.95 + swell * 0.3),
        drift + travelDir * (0.7 + bass * 1.2 + swell * 0.5)
    );

    float zoom = 1.0 + bass * 0.4 + v * 0.3 + iAudioBeatPulse * 0.25;
    vec3 rd = normalize(vec3(uv * 0.85, zoom));
    rd.xy = rot2(time * 0.06 + v * 0.15 + momentum * 0.3) * rd.xy;

    vec3 col = vec3(0.0);
    float travel = 0.0;

    for (int i = 0; i < 70; i++) {
        vec3 p = ro + rd * travel * travelDir;

        // Subtle flow field distortion
        float sw = time * 0.18 + dot(p, vec3(0.05, 0.07, 0.09));
        p += vec3(sin(sw), cos(sw), 0.0) * (0.1 + abs(totalFlow) * 0.15);
        p.z += bass * 0.2;

        vec3 cell = floor(p);
        vec3 local = fract(p) - 0.5;

        // Per-cell audio: drives SIZE
        float freqIdx = fract(dot(cell, vec3(0.31, 0.21, 0.13)) * 0.25);
        float amp = pitch(freqIdx, 1.0 + v * 0.3);
        float size = 0.18 + amp * 0.25 + iAudioMid * 0.08;

        float d = sdBox(local, vec3(size));
        float dist = max(d, 0.0);

        // Glow from SDF distance — brightness is geometric, not audio-driven
        float sparkle = exp(-dist * 16.0) / (0.5 + dist * 32.0);
        vec3 sc = scheme(cell + vec3(0.0, 0.0, drift * 0.25), time * cs * 0.5 + freqIdx * 2.0);
        col += sc * sparkle * 0.12 * g * 0.5;

        travel += max(abs(d), 0.04);
        if (travel > 34.0) break;
    }

    return col;
}

// ─── Style 0: Grid ───────────────────────────────────────────
// Infinite flying grid of reactive cubes with positional twist.
// Audio drives box size and flight speed — not glow intensity.

vec3 gridStyle(vec2 uv, float time) {
    float v = vol();
    float swell = clamp(iAudioSwell, 0.0, 1.0);
    float momentum = clamp(iAudioMomentum, -1.0, 1.0);
    float fl = flowN();
    float cs = cSpd();
    float g = glowN();

    float flow = clamp(fl + momentum * 0.4, -1.8, 1.8);
    float flowDir = flow >= 0.0 ? -1.0 : 1.0;
    float speed = 1.4 + abs(flow) * 2.0 + swell * 1.0 + v * 0.7;
    float beatDrive = 1.0 + iAudioBeatPulse * 0.5;

    vec3 rd = normalize(vec3(uv * (0.75 + v * 0.2), flowDir * (1.1 + swell * 0.4)));
    rd.xy = rot2(time * 0.08 + momentum * 0.3) * rd.xy;

    vec3 ro = vec3(0.0, 0.0, time * speed * flowDir * beatDrive);
    ro.xy += vec2(sin(time * 0.25), cos(time * 0.18)) * (0.3 + v * 0.4);
    ro.xy += rot2(time * 0.05) * vec2(momentum * 0.8, swell * 0.5);

    vec3 col = vec3(0.0);

    for (float i = 0.0, t = 0.0; i < 60.0; i++) {
        vec3 p = ro + t * rd;
        p.xy = rot2(sin(time * 0.04 + p.z * 0.3) * 0.2) * p.xy;

        vec3 id = floor(p);
        vec3 q = fract(p) - 0.5;

        float freqIdx = mod(abs(id.x) + abs(id.y) * 2.0 + abs(id.z) * 0.5, 32.0) / 32.0;
        float amp = pitch(freqIdx, 1.0 + v * 0.3);

        // Audio drives box SIZE
        float boxSize = 0.22 + amp * 0.22;
        float d = sdBox(q, vec3(boxSize));
        float fade = exp(-t * 0.05);

        // Geometric glow — brightness from SDF distance, not audio amplitude
        float crisp = exp(-max(d, 0.0) * 20.0);
        float edge = smoothstep(0.15, 0.0, abs(d)) * 0.35;
        float glow = (crisp * 0.8 + edge * 0.4) * 0.5;

        vec3 sc = scheme(id, time * cs * 0.5 + freqIdx * 3.0);
        col += sc * glow * fade * g * 0.45;

        t += max(abs(d), 0.04) / (beatDrive * (1.0 + abs(flow) * 0.3));
        if (t > 35.0) break;
    }

    return col;
}

// ─── Style 3: Waveform ───────────────────────────────────────
// Layered frequency ribbons scrolling horizontally.
// Audio drives wave height and phase — not band brightness.

vec3 waveformStyle(vec2 uv, float time) {
    float swell = clamp(iAudioSwell, 0.0, 1.0);
    float momentum = clamp(iAudioMomentum, -1.0, 1.0);
    float fl = flowN();
    float cs = cSpd();
    float g = glowN();

    vec2 w = uv;
    w.x += time * (fl * 0.45 + momentum * 0.35);

    vec3 col = vec3(0.0);

    for (int layer = 0; layer < 4; layer++) {
        float fi = float(layer);
        float layerDepth = 1.0 - fi * 0.22;
        float layerTime = time * (0.7 + fi * 0.12);
        float freqIdx = fract((w.x * 0.35 + layerTime * 0.2) + fi * 0.17);
        float amp = pitch(freqIdx, 1.0 + swell * 0.5 + fi * 0.2);

        // Audio drives wave DISPLACEMENT
        float crest = sin(w.x * (3.4 + fi * 0.7) + layerTime * 2.0 + iAudioBeatPulse * 3.0);
        float height = 0.22 + swell * 0.25 + fi * 0.1 + amp * 0.45;
        float y = crest * height - fi * 0.32;
        y += sin(w.x * 1.5 + time * 0.5 + fi) * momentum * 0.18;

        // Band glow — stable brightness, audio drives position
        float band = exp(-abs(w.y - y) * 200.0) * 0.5;
        float sharp = exp(-abs(w.y - y) * 400.0) * 0.25;

        vec3 sc = scheme(vec3(fi * 1.5, freqIdx * 10.0, layerTime), time * cs * 0.5 + fi * 0.2);
        col += sc * (band + sharp) * layerDepth * g * 0.7;
    }

    // Background gradient
    vec3 bgLow = scheme(vec3(-2.0, 0.0, 0.0), time * 0.1);
    vec3 bgHigh = scheme(vec3(4.0, 1.5, 0.0), time * -0.05);
    col += mix(bgLow, bgHigh, smoothstep(-0.8, 0.9, w.y)) * 0.04;

    return col;
}

// ─── Style 2: Vortex ─────────────────────────────────────────
// Spiral tunnel with frequency-reactive rings.
// Audio drives ring position and rotation speed — not ring brightness.

vec3 vortexStyle(vec2 uv, float time) {
    float v = vol();
    float swell = clamp(iAudioSwell, 0.0, 1.0);
    float momentum = clamp(iAudioMomentum, -1.0, 1.0);
    float bass = clamp(iAudioBass * sens(), 0.0, 1.5);
    float fl = flowN();
    float cs = cSpd();
    float g = glowN();

    float flow = clamp(fl + momentum * 0.5, -1.5, 1.5);
    float swirlDir = flow >= 0.0 ? 1.0 : -1.0;
    float swirlSpeed = 0.4 + abs(flow) * 0.8 + swell * 0.5;

    vec2 su = rot2(time * swirlSpeed * swirlDir + iAudioBeatPulse * 0.35 + v * 0.2) * uv;
    float r = length(su);
    float a = atan(su.y, su.x);

    vec3 col = vec3(0.0);

    // Frequency-reactive rings
    for (int i = 0; i < 8; i++) {
        float fi = float(i);
        float freqIdx = fi / 7.0;
        float amp = max(0.05, pitch(freqIdx, 1.0 + iAudioTreble * 0.6 + swell * 0.5));

        // Audio drives ring POSITION
        float ringR = 0.12 + fi * 0.12 + amp * 0.2 + v * 0.1 + bass * 0.08;
        ringR += sin(time * 0.4 + fi * 0.6 + iAudioBeatPulse * 1.0) * (0.03 + swell * 0.05);

        float spiral = sin(fract(a / TAU + fi * 0.11 - time * swirlSpeed * swirlDir * 0.5) * TAU + flow * 2.0)
                      * (0.05 + amp * 0.07);
        float d = abs(r - ringR - spiral);

        // Ring glow — stable brightness
        float ringGlow = exp(-d * d * 160.0) * 0.55;
        ringGlow *= 0.5 + pitch(fract(a / TAU * 12.0 + fi * 0.3 + time * 0.35), 0.9);

        vec3 sc = scheme(vec3(fi * 1.2, ringR * 7.0, 0.0), time * cs * 0.5 + freqIdx * 1.8);
        col += sc * ringGlow * g * 0.7;
    }

    // Spiral arms
    float armPattern = sin(a * 8.0 - time * (1.1 + iAudioTreble) + flow * 2.0) * 0.5 + 0.5;
    float armGlow = exp(-abs(armPattern - r * 0.8) * 5.5) * exp(-r * 1.4);
    col += scheme(vec3(8.0, 0.5, 0.0), time * 0.45) * armGlow * 0.4;

    // Center glow
    float center = exp(-r * 3.6) * 0.6;
    col += scheme(vec3(0.0, 3.0, 0.0), time * 0.9) * center * g * 0.6;

    // Rim falloff
    col *= 1.0 - smoothstep(0.6, 1.6, r) * 0.3;

    return col;
}

// ─── Main ────────────────────────────────────────────────────

void mainImage(out vec4 fragOut, vec2 fragCoord) {
    vec2 uv = (2.0 * fragCoord - iResolution.xy) / iResolution.y;

    vec3 color;
    if (iVisualStyle == 0) {
        color = gridStyle(uv, iTime);
    } else if (iVisualStyle == 1) {
        color = pulseFieldStyle(uv, iTime);
    } else if (iVisualStyle == 2) {
        color = vortexStyle(uv, iTime);
    } else {
        color = waveformStyle(uv, iTime);
    }

    // Tone map
    color = color / (1.0 + color * 0.5);
    color = satBoost(color, 0.1);

    fragOut = vec4(clamp(color, 0.0, 1.0), 1.0);
}

void main() {
    mainImage(fragColor, gl_FragCoord.xy);
}
