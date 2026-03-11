#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iAudioBass;
uniform float iAudioMid;
uniform float iAudioTreble;
uniform float iAudioBeat;
uniform float iAudioBeatPulse;
uniform float iAudioLevel;
uniform float iAudioSwell;
uniform float iAudioSpectralFlux;

uniform float iSpeed;
uniform float iIntensity;
uniform float iRingCount;
uniform float iDecay;
uniform int iPalette;
uniform int iScene;

// ── LED-safe palettes ──────────────────────────────────────────────
// Tier 1/2 hues only.  Saturation ≥ 85 %.  Whiteness ratio < 0.25.
// At least one RGB channel is near zero in every entry.

vec3 ledPrimary(int pal) {
    if (pal == 0) return vec3(0.88, 0.21, 1.00); // SilkCircuit – electric purple
    if (pal == 1) return vec3(1.00, 0.05, 0.78); // Cyberpunk   – hot magenta
    if (pal == 2) return vec3(1.00, 0.04, 0.02); // Fire        – pure red
    if (pal == 3) return vec3(0.04, 1.00, 0.32); // Aurora      – vivid green
    return vec3(0.08, 0.38, 1.00);               // Ice         – deep sapphire
}

vec3 ledSecondary(int pal) {
    if (pal == 0) return vec3(0.00, 1.00, 0.88); // neon cyan
    if (pal == 1) return vec3(0.04, 0.18, 1.00); // deep blue
    if (pal == 2) return vec3(1.00, 0.38, 0.00); // orange
    if (pal == 3) return vec3(0.00, 0.84, 0.72); // teal
    return vec3(0.00, 0.88, 1.00);               // bright cyan
}

vec3 ledAccent(int pal) {
    if (pal == 0) return vec3(1.00, 0.42, 0.76); // coral
    if (pal == 1) return vec3(0.00, 0.92, 1.00); // electric cyan
    if (pal == 2) return vec3(1.00, 0.56, 0.00); // amber
    if (pal == 3) return vec3(0.58, 0.14, 1.00); // violet
    return vec3(0.30, 0.06, 0.96);               // indigo
}

// Blend in squared space — avoids sRGB midpoint muddiness.
vec3 paletteAt(float t, int pal) {
    t = fract(t) * 3.0;
    float f = fract(t);
    vec3 a, b;
    if (t < 1.0)      { a = ledPrimary(pal);   b = ledSecondary(pal); }
    else if (t < 2.0)  { a = ledSecondary(pal); b = ledAccent(pal); }
    else               { a = ledAccent(pal);    b = ledPrimary(pal); }
    return sqrt(mix(a * a, b * b, f));
}

// ── Emitter configuration per scene ────────────────────────────────

int emitterCount(int sc) {
    if (sc == 1) return 2;
    if (sc == 2) return 3;
    return 1;
}

vec2 emitterPos(int sc, int idx) {
    if (sc == 1) return idx == 0 ? vec2(-0.35, 0.0) : vec2(0.35, 0.0);
    if (sc == 2) {
        if (idx == 0) return vec2(0.0, 0.28);
        if (idx == 1) return vec2(-0.26, -0.16);
        return vec2(0.26, -0.16);
    }
    return vec2(0.0);
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    vec2 p  = uv * 2.0 - 1.0;
    p.x *= iResolution.x / iResolution.y;

    float speedN     = clamp(iSpeed / 10.0, 0.0, 1.0);
    float intensityN = clamp(iIntensity * 0.01, 0.0, 1.0);
    float ringN      = clamp(iRingCount * 0.01, 0.0, 1.0);
    float decayN     = clamp(iDecay * 0.01, 0.0, 1.0);

    float bass   = clamp(iAudioBass, 0.0, 1.0);
    float beat   = clamp(iAudioBeatPulse, 0.0, 1.0);
    float level  = clamp(iAudioLevel, 0.0, 1.0);
    float treble = clamp(iAudioTreble, 0.0, 1.0);
    float swell  = clamp(iAudioSwell, 0.0, 1.0);

    float t = iTime * (0.32 + iSpeed * 0.68);

    // ── Fallback pulse (keeps motion alive without audio) ──────────
    float fallbackBeat  = pow(max(0.0, sin(t * 1.7)), 8.0);
    float fallbackSwell = 0.5 + 0.5 * sin(t * 0.55);
    float audioPresence = smoothstep(0.02, 0.14, level + bass + beat);
    float pulse  = mix(fallbackBeat, max(beat, bass * 0.9), audioPresence);
    float energy = mix(0.4 + fallbackSwell * 0.4,
                       clamp(level + bass * 0.5, 0.0, 1.3),
                       audioPresence);

    int sc       = clamp(iScene, 0, 2);
    int emitters = emitterCount(sc);

    // ── Ring parameters — wide, LED-readable ───────────────────────
    float totalRings  = floor(mix(2.0, 6.0, ringN));
    float ringsPerE   = max(2.0, totalRings / float(emitters));
    float decayRate   = mix(1.2, 4.5, decayN);
    float ringSpeed   = mix(0.18, 0.55, speedN);
    // 10-20× wider than the original thin lines
    float ringWidth   = mix(0.18, 0.10, ringN) * (1.0 + bass * 0.25);

    vec3 col = vec3(0.0);

    // ── Expanding rings from each emitter ──────────────────────────
    for (int e = 0; e < 3; e++) {
        if (e >= emitters) break;
        vec2 origin = emitterPos(sc, e);
        float r = length(p - origin);

        for (int i = 0; i < 8; i++) {
            float fi = float(i);
            if (fi >= ringsPerE) break;

            float phase  = fract(t * ringSpeed + float(e) * 0.29 + fi / ringsPerE);
            float radius = phase * 1.5;

            // Gaussian ring profile — broad and smooth
            float dist = r - radius;
            float w    = ringWidth * (1.0 + radius * 0.2); // widens as it expands
            float ring = exp(-dist * dist / (w * w));

            // Life: exponential fade with age
            float life = exp(-phase * decayRate);

            // Birth flash: bright pulse when ring is born
            float birth = smoothstep(0.06, 0.0, phase) * (0.5 + pulse * 1.2);
            ring *= life * (1.0 + birth);

            // Color varies by ring index, emitter, and time
            float tone = fi / max(ringsPerE, 1.0)
                       + float(e) * 0.20
                       + t * 0.03
                       + treble * 0.08;
            vec3 ringColor = paletteAt(tone, iPalette);

            // Screen blend: bounded (never exceeds 1.0), preserves saturation
            vec3 contrib = ringColor * ring;
            col = col + contrib * (1.0 - col);
        }
    }

    // ── Core glow — vivid palette color, never white ───────────────
    for (int e = 0; e < 3; e++) {
        if (e >= emitters) break;
        float coreR = length(p - emitterPos(sc, e));
        float core  = exp(-coreR * coreR / 0.035);
        core *= 0.4 + pulse * 0.9;
        vec3 coreColor = ledPrimary(iPalette) * core;
        col = col + coreColor * (1.0 - col);
    }

    // ── Scene accents — broad features only ────────────────────────
    if (sc == 1) {
        // Twin Burst: wide connecting band between emitters
        float band = exp(-p.y * p.y / 0.03);
        band *= smoothstep(0.55, 0.0, abs(p.x));
        band *= 0.15 + pulse * 0.35;
        vec3 bandColor = ledAccent(iPalette) * band;
        col = col + bandColor * (1.0 - col);
    } else if (sc == 2) {
        // Triad: gentle three-fold rotational sweep
        float a     = atan(p.y, p.x);
        float sweep = smoothstep(0.75, 1.0, sin(a * 3.0 + t * 1.8));
        sweep *= exp(-length(p) * 1.2);
        sweep *= 0.10 + pulse * 0.25;
        vec3 sweepColor = ledSecondary(iPalette) * sweep;
        col = col + sweepColor * (1.0 - col);
    }

    // ── Intensity + energy ─────────────────────────────────────────
    float brightness = mix(0.35, 1.2, intensityN) * mix(0.8, 1.2, energy);
    col *= brightness;

    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
