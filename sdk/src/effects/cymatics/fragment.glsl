#version 300 es
// Cymatics — Cinematic Audio Visualizer
// Sound made visible. Four modes: Lattice, Pulse Field, Vortex, Resonance.
// Inspired by "3D Audio Visualizer" by @kishimisu (CC BY-NC-SA 4.0)
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
uniform float iAudioLevelSmooth;
uniform float iAudioBassSmooth;
uniform float iAudioMidSmooth;
uniform float iAudioTrebleSmooth;
uniform float iMotionEnergy;
uniform float iMotionPulse;
uniform vec2 iMotionPan;
uniform float iMotionZoom;
uniform float iMotionTwist;
uniform float iFlowDrift;
uniform float iWarpPhase;

#define PI 3.14159265359
#define TAU 6.28318530718

// ─── Normalized controls ─────────────────────────────────────

float sens()  { return clamp(iSensitivity / 100.0, 0.1, 2.0); }
float flowN() { return clamp(iFlow / 100.0, -1.0, 1.0); }
float glowN() { return clamp(iGlowIntensity / 100.0, 0.1, 2.0); }
float cSpd()  { return clamp(iColorSpeed / 100.0, 0.0, 2.0); }

// ─── Audio helpers ───────────────────────────────────────────

float levelEnv()  { return clamp(iAudioLevelSmooth, 0.0, 1.0); }
float bassEnv()   { return clamp(iAudioBassSmooth, 0.0, 1.0); }
float midEnv()    { return clamp(iAudioMidSmooth, 0.0, 1.0); }
float trebleEnv() { return clamp(iAudioTrebleSmooth, 0.0, 1.0); }
float motionEnergy() { return clamp(iMotionEnergy, 0.0, 1.0); }
float motionPulse() { return clamp(iMotionPulse, 0.0, 1.0); }

// Interpolate bass/mid/treble into a pseudo-spectrum
float getFreq(float idx) {
    idx = clamp(idx, 0.0, 1.0);
    float bW = exp(-idx * idx * 30.0);
    float mW = exp(-(idx - 0.35) * (idx - 0.35) * 12.0);
    float tW = exp(-(idx - 0.75) * (idx - 0.75) * 10.0);
    float total = bW + mW + tW + 0.001;
    return (bassEnv() * bW + midEnv() * mW + trebleEnv() * tW) / total;
}

float pitch(float freq, float scale) {
    return clamp(getFreq(freq) * sens() * scale, 0.0, 1.0);
}

float vol() { return clamp(levelEnv() * sens(), 0.0, 1.2); }

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
// Three-color cycling for richer palettes. Each scheme has an anchor,
// complement, and accent that cycle at different rates for organic
// color diversity without monotone flatness.
// Combo indices: Aurora(0), Cyberpunk(1), Lava(2), Prism(3), Toxic(4), Vaporwave(5)

vec3 scheme(vec3 id, float t) {
    vec3 a, b, c;
    if (iColorScheme == 1) {         // Cyberpunk
        a = vec3(0.98, 0.05, 0.78); b = vec3(0.05, 0.82, 1.15); c = vec3(0.92, 0.48, 0.08);
    } else if (iColorScheme == 2) {  // Lava
        a = vec3(1.2, 0.15, 0.03);  b = vec3(1.0, 0.55, 0.04);  c = vec3(0.85, 0.18, 0.65);
    } else if (iColorScheme == 0) {  // Aurora
        a = vec3(0.1, 0.85, 0.6);   b = vec3(0.25, 0.35, 1.2);  c = vec3(0.72, 0.25, 0.92);
    } else if (iColorScheme == 5) {  // Vaporwave
        a = vec3(1.1, 0.35, 0.85);  b = vec3(0.3, 0.85, 1.15);  c = vec3(0.85, 0.72, 0.2);
    } else if (iColorScheme == 4) {  // Toxic
        a = vec3(0.3, 1.15, 0.15);  b = vec3(0.05, 0.6, 0.95);  c = vec3(0.88, 0.95, 0.08);
    } else {                         // Prism (full rainbow)
        return satBoost(0.6 + 0.6 * cos(id * 0.8 + vec3(0.0, 2.0, 4.0) + t), 0.9);
    }
    // Three-phase cycling — a↔b at one rate, blend toward c at another
    float phase1 = 0.5 + 0.5 * sin(t + dot(id, vec3(0.7, 0.3, 0.55)));
    float phase2 = 0.5 + 0.5 * sin(t * 0.73 + dot(id, vec3(0.35, 0.6, 0.25)) + 2.1);
    vec3 ab = mix(a, b, phase1);
    vec3 col = mix(ab, c, phase2 * 0.45);
    col += 0.12 * sin(vec3(0.0, 2.0, 4.0) + t * 0.65 + id.yzx * 0.7);
    return satBoost(col, 0.85);
}

// ─── Style 1: Particle Field ────────────────────────────────
// Ethereal lattice of glowing cubes flowing through 3D space.
// Audio drives cube size and camera movement — not brightness.

vec3 pulseFieldStyle(vec2 uv, float time) {
    float v = vol();
    float energy = motionEnergy();
    float pulse = motionPulse();
    float swell = clamp(iAudioSwell * 0.65 + pulse * 0.25, 0.0, 1.0);
    float momentum = clamp(iAudioMomentum * 0.8, -1.0, 1.0);
    float bass = clamp(bassEnv() * sens(), 0.0, 1.5);
    float mid = clamp(midEnv() * sens(), 0.0, 1.3);
    float fl = flowN();
    float cs = cSpd();
    float g = glowN();
    float zoom = max(iMotionZoom, 0.82);

    // Flow direction with audio momentum
    float totalFlow = clamp(fl + momentum * 0.42 + pulse * 0.12 * sign(fl + momentum + 0.001), -1.8, 1.8);
    float travelDir = totalFlow >= 0.0 ? -1.0 : 1.0;

    // Camera — audio drives position and speed
    float drift = time * totalFlow * (1.2 + swell * 0.95 + v * 0.35 + energy * 0.55 + pulse * 0.3) + iFlowDrift * 3.4;
    vec3 ro = vec3(
        sin(time * 0.20 + iWarpPhase * 0.22) * (0.82 + swell * 0.32 + v * 0.18 + energy * 0.26 + pulse * 0.18) + iMotionPan.x * 3.1,
        cos(time * 0.17 - iWarpPhase * 0.18) * (0.64 + mid * 0.18 + energy * 0.18) + iMotionPan.y * 2.6,
        drift + travelDir * (0.58 + bass * 0.92 + pulse * 0.42 + energy * 0.28)
    );

    vec2 view = (uv + iMotionPan * 0.22) * ((0.78 - energy * 0.06) / zoom);
    vec3 rd = normalize(vec3(view, 0.9 + bass * 0.18 + v * 0.1 + pulse * 0.08));
    rd.xy = rot2(time * 0.06 + iMotionTwist * 0.72 + momentum * 0.16 + energy * 0.08) * rd.xy;

    vec3 col = vec3(0.0);
    float travel = 0.0;

    for (int i = 0; i < 70; i++) {
        vec3 p = ro + rd * travel * travelDir;

        // Subtle flow field distortion
        float sw = time * 0.15 + iWarpPhase * 0.5 + dot(p, vec3(0.05, 0.07, 0.09));
        p += vec3(sin(sw), cos(sw + iWarpPhase * 0.35), 0.0) * (0.1 + abs(totalFlow) * 0.12 + energy * 0.06 + pulse * 0.08);
        p.z += bass * 0.14 + sin(iWarpPhase + p.x * 0.4) * (0.05 + energy * 0.03 + pulse * 0.02);

        vec3 cell = floor(p);
        vec3 local = fract(p) - 0.5;

        // Per-cell audio: drives SIZE
        float freqIdx = fract(dot(cell, vec3(0.31, 0.21, 0.13)) * 0.25);
        float amp = pitch(freqIdx, 1.0 + v * 0.3);
        float size = 0.18 + amp * 0.22 + mid * 0.08 + pulse * 0.04 + energy * 0.03;

        float d = sdBox(local, vec3(size));
        float dist = max(d, 0.0);

        // Glow from SDF distance — brightness is geometric, not audio-driven
        float sparkle = exp(-dist * 18.0) / (0.7 + dist * 30.0);
        vec3 sc = scheme(cell + vec3(0.0, 0.0, drift * 0.25), time * cs * 0.5 + freqIdx * 2.0);
        col += sc * sparkle * 0.1 * g * (0.46 + energy * 0.08);

        travel += max(abs(d), 0.038) / (1.0 + energy * 0.12 + pulse * 0.08);
        if (travel > 34.0) break;
    }

    return col;
}

// ─── Style 0: Lattice ────────────────────────────────────────
// Infinite flying lattice of reactive cubes with positional twist.
// Audio drives box size and flight speed — not glow intensity.

vec3 gridStyle(vec2 uv, float time) {
    float v = vol();
    float energy = motionEnergy();
    float pulse = motionPulse();
    float swell = clamp(iAudioSwell * 0.65 + pulse * 0.22, 0.0, 1.0);
    float momentum = clamp(iAudioMomentum * 0.8, -1.0, 1.0);
    float fl = flowN();
    float cs = cSpd();
    float g = glowN();
    float zoom = max(iMotionZoom, 0.82);

    float flow = clamp(fl + momentum * 0.42 + pulse * 0.18 * sign(fl + 0.001), -1.8, 1.8);
    float flowDir = flow >= 0.0 ? -1.0 : 1.0;
    float speed = 1.35 + abs(flow) * 1.6 + swell * 0.82 + v * 0.5 + energy * 0.8 + pulse * 0.45;
    float drive = 0.95 + pulse * 0.24 + bassEnv() * 0.14 + energy * 0.18;

    vec2 view = (uv + iMotionPan * 0.28) * ((0.7 + v * 0.1 + energy * 0.06) / zoom);
    vec3 rd = normalize(vec3(view, flowDir * (1.02 + swell * 0.18)));
    rd.xy = rot2(time * 0.08 + iMotionTwist * 0.56 + momentum * 0.14 + pulse * 0.08) * rd.xy;

    vec3 ro = vec3(iMotionPan.x * 2.6, iMotionPan.y * 2.2, time * speed * flowDir + iFlowDrift * 2.8);
    ro.xy += vec2(sin(time * 0.21 + iWarpPhase * 0.25), cos(time * 0.16 - iWarpPhase * 0.2)) * (0.28 + v * 0.18 + energy * 0.12);
    ro.xy += rot2(time * 0.06 + pulse * 0.25) * vec2(momentum * 0.52 + pulse * 0.18, swell * 0.34 + energy * 0.12);

    vec3 col = vec3(0.0);

    for (float i = 0.0, t = 0.0; i < 60.0; i++) {
        vec3 p = ro + t * rd;
        p.xy = rot2(sin(time * 0.05 + p.z * 0.32 + pulse * 0.6) * (0.24 + energy * 0.06)) * p.xy;

        vec3 id = floor(p);
        vec3 q = fract(p) - 0.5;

        float freqIdx = mod(abs(id.x) + abs(id.y) * 2.0 + abs(id.z) * 0.5, 32.0) / 32.0;
        float amp = pitch(freqIdx, 1.0 + v * 0.3);

        // Audio drives box SIZE
        float boxSize = 0.2 + amp * 0.22 + pulse * 0.05 + energy * 0.03;
        float d = sdBox(q, vec3(boxSize));
        float fade = exp(-t * 0.05);

        // Geometric glow — brightness from SDF distance, not audio amplitude
        float crisp = exp(-max(d, 0.0) * 20.0);
        float edge = smoothstep(0.15, 0.0, abs(d)) * 0.35;
        float glow = (crisp * 0.75 + edge * 0.35) * (0.45 + energy * 0.08);

        vec3 sc = scheme(id, time * cs * 0.5 + freqIdx * 3.0);
        col += sc * glow * fade * g * 0.45;

        t += max(abs(d), 0.042) / (drive * (1.0 + abs(flow) * 0.28 + energy * 0.12 + pulse * 0.08));
        if (t > 35.0) break;
    }

    return col;
}

// ─── Style 3: Resonance ─────────────────────────────────────
// 8 frequency ribbons with multi-octave organic wave shapes,
// bass impact ripples, variable ribbon width, and spatial audio
// modulation. Each layer responds to a different frequency band
// with depth parallax and multi-glow rendering.

vec3 waveformStyle(vec2 uv, float time) {
    float energy = motionEnergy();
    float pulse = motionPulse();
    float swell = clamp(iAudioSwell * 0.7 + pulse * 0.18, 0.0, 1.0);
    float momentum = clamp(iAudioMomentum * 0.85, -1.0, 1.0);
    float bass = bassEnv();
    float mid = midEnv();
    float treble = trebleEnv();
    float fl = flowN();
    float cs = cSpd();
    float g = glowN();
    float zoom = max(iMotionZoom, 0.82);

    vec2 w = rot2(iMotionTwist * 0.18 + momentum * 0.08) * uv;
    w += iMotionPan * vec2(0.6, 0.42);
    w /= max(zoom * (1.0 - energy * 0.08), 0.7);
    float scrollX = time * (fl * 0.55 + momentum * 0.32) * (1.0 + energy * 0.35 + pulse * 0.25) + iFlowDrift * 0.42;
    w.x += scrollX;
    w.y += sin(iWarpPhase * 0.35 + time * (0.25 + energy * 0.18)) * (0.06 + energy * 0.03);

    vec3 col = vec3(0.0);

    for (int layer = 0; layer < 8; layer++) {
        float fi = float(layer);
        float depth = 1.0 - fi * 0.10;
        float bandCenter = fi / 7.0;
        float bandAmp = pitch(bandCenter, 1.0 + swell * 0.5 + fi * 0.15);

        // Parallax-shifted x for depth separation
        float wx = w.x * (0.85 + fi * 0.04) + fi * 1.7;
        float lt = time * (0.6 + fi * 0.12) * (1.0 + energy * 0.2 + pulse * 0.15);

        // Multi-octave organic wave — each harmonic responds to a different band
        float wave = 0.0;
        wave += sin(wx * 1.8 + lt * 1.2 + bass * 2.8) * (0.35 + bass * 0.25);       // Fundamental — bass swell
        wave += sin(wx * 3.7 - lt * 0.9 + fi * 1.1 + mid * 1.5) * (0.22 + mid * 0.15); // Body — mid-range
        wave += sin(wx * 7.3 + lt * 2.2 + fi * 2.5) * (0.08 + treble * 0.18);        // Shimmer — treble
        wave += sin(wx * 0.9 - lt * 0.4 + iWarpPhase * 0.6) * (0.18 + swell * 0.12); // Sub-octave — slow rolling
        wave += sin(wx * 13.0 + lt * 3.5 + fi * 4.2) * pulse * 0.12;                 // Jitter — onset energy

        // Bass impact ripple — propagates outward from beat events
        float impactPhase = wx * 2.5 - pulse * 8.0;
        wave += sin(impactPhase) * exp(-abs(impactPhase) * 0.3) * pulse * 0.4;

        // Vertical positioning — spread layers across the viewport
        float yCenter = (fi - 3.5) * 0.28;
        float y = wave * (0.12 + bandAmp * 0.18 + energy * 0.04) + yCenter;

        float dist = abs(w.y - y);

        // Dynamic ribbon width — bass layers wider, treble thinner
        float ribbonWidth = (0.026 - fi * 0.002) + bandAmp * 0.018 + bass * 0.008 * (1.0 - bandCenter);

        // Multi-glow: sharp core + medium bloom + soft atmosphere
        float core = exp(-dist * dist / (ribbonWidth * ribbonWidth)) * 0.55;
        float bloom = exp(-dist * (50.0 - energy * 12.0)) * 0.22;
        float atmosphere = exp(-dist * (12.0 - energy * 3.0)) * 0.06;

        // Audio energy modulates brightness spatially along the ribbon
        float xEnergy = pitch(fract(wx * 0.08 + lt * 0.05), 0.8);
        float energyMod = 0.7 + xEnergy * 0.45 + pulse * 0.15;

        // Peak crests glow brighter — rewards big wave displacement
        float crestBoost = smoothstep(0.0, 0.3, abs(wave)) * 0.25;

        float totalGlow = (core + bloom + atmosphere) * energyMod * (1.0 + crestBoost);

        vec3 sc = scheme(vec3(fi * 1.5, bandCenter * 8.0, lt * 0.3), time * cs * 0.5 + fi * 0.35);
        col += sc * totalGlow * depth * g * 0.65;
    }

    // Subtle background gradient
    vec3 bgLow = scheme(vec3(-2.0, 0.0, 0.0), time * 0.1);
    vec3 bgHigh = scheme(vec3(4.0, 1.5, 0.0), time * -0.05);
    col += mix(bgLow, bgHigh, smoothstep(-1.2, 1.2, w.y)) * 0.03;

    return col;
}

// ─── Style 2: Vortex ─────────────────────────────────────────
// Spiral tunnel with frequency-reactive rings.
// Audio drives ring position and rotation speed — not ring brightness.

vec3 vortexStyle(vec2 uv, float time) {
    float v = vol();
    float energy = motionEnergy();
    float pulse = motionPulse();
    float swell = clamp(iAudioSwell * 0.7 + pulse * 0.22, 0.0, 1.0);
    float momentum = clamp(iAudioMomentum * 0.82, -1.0, 1.0);
    float bass = clamp(bassEnv() * sens(), 0.0, 1.5);
    float fl = flowN();
    float cs = cSpd();
    float g = glowN();
    float zoom = max(iMotionZoom, 0.82);

    float flow = clamp(fl + momentum * 0.56 + pulse * 0.18 * sign(fl + momentum + 0.001), -1.7, 1.7);
    float swirlDir = flow >= 0.0 ? 1.0 : -1.0;
    float swirlSpeed = 0.42 + abs(flow) * 0.7 + swell * 0.42 + energy * 0.25 + pulse * 0.35;

    vec2 su = rot2(time * swirlSpeed * swirlDir + iMotionTwist * 0.88 + iWarpPhase * 0.24 + v * 0.1 + energy * 0.12)
            * ((uv + iMotionPan * 0.24) / (zoom * (1.0 + pulse * 0.05)));
    float r = length(su);
    float a = atan(su.y, su.x);

    vec3 col = vec3(0.0);

    // Frequency-reactive rings
    for (int i = 0; i < 8; i++) {
        float fi = float(i);
        float freqIdx = fi / 7.0;
        float amp = max(0.05, pitch(freqIdx, 1.0 + trebleEnv() * 0.4 + swell * 0.35));

        // Audio drives ring POSITION
        float ringR = 0.12 + fi * 0.11 + amp * 0.18 + v * 0.08 + bass * 0.08 + energy * 0.05;
        ringR += sin(time * 0.35 + fi * 0.55 + iWarpPhase * (0.85 + freqIdx * 0.3)) * (0.03 + swell * 0.05 + energy * 0.03 + pulse * 0.03);

        float spiral = sin(fract(a / TAU + fi * 0.11 - time * swirlSpeed * swirlDir * 0.5) * TAU + flow * 2.0)
                      * (0.05 + amp * 0.07 + energy * 0.02);
        float d = abs(r - ringR - spiral);

        // Ring glow — stable brightness
        float ringGlow = exp(-d * d * (130.0 - energy * 20.0)) * 0.48;
        ringGlow *= 0.55 + pitch(fract(a / TAU * 10.0 + fi * 0.25 + iFlowDrift * 0.08), 0.78);

        vec3 sc = scheme(vec3(fi * 1.2, ringR * 7.0, 0.0), time * cs * 0.5 + freqIdx * 1.8);
        col += sc * ringGlow * g * 0.7;
    }

    // Spiral arms
    float armPattern = sin(a * 8.4 - time * (1.1 + trebleEnv() * 0.5 + energy * 0.2) + iWarpPhase * 1.2 + flow * 1.4) * 0.5 + 0.5;
    float armGlow = exp(-abs(armPattern - r * 0.8) * 5.5) * exp(-r * 1.4);
    col += scheme(vec3(8.0, 0.5, 0.0), time * 0.45) * armGlow * 0.4;

    // Center glow
    float center = exp(-r * (3.6 - pulse * 0.45 - energy * 0.18)) * 0.56;
    col += scheme(vec3(0.0, 3.0, 0.0), time * 0.9) * center * g * 0.6;

    // Rim falloff
    col *= 1.0 - smoothstep(0.65, 1.65, r) * 0.25;

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
    color = color / (1.0 + color * 0.65);
    color = satBoost(color, 0.08);

    fragOut = vec4(clamp(color, 0.0, 1.0), 1.0);
}

void main() {
    mainImage(fragColor, gl_FragCoord.xy);
}
