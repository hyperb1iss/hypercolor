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

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
    if (id == 1) return iqPalette(t, vec3(0.5, 0.2, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 1.0), vec3(0.8, 0.1, 0.6));
    if (id == 2) return iqPalette(t, vec3(0.5, 0.2, 0.0), vec3(0.5, 0.4, 0.2), vec3(1.0, 0.7, 0.4), vec3(0.0, 0.15, 0.2));
    if (id == 3) return iqPalette(t, vec3(0.2, 0.5, 0.4), vec3(0.3, 0.4, 0.4), vec3(0.8, 0.7, 0.9), vec3(0.6, 0.3, 0.7));
    if (id == 4) return iqPalette(t, vec3(0.5, 0.6, 0.8), vec3(0.2, 0.3, 0.2), vec3(0.6, 0.8, 1.0), vec3(0.0, 0.1, 0.3));
    return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
}

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

float ringProfile(float dist, float width) {
    float core = smoothstep(width, width * 0.10, dist);
    float halo = smoothstep(width * 3.6, width * 0.7, dist) * 0.28;
    return core * 1.35 + halo;
}

int sceneEmitterCount(int mode) {
    if (mode == 1) return 2;
    if (mode == 2) return 3;
    return 1;
}

vec2 emitterPos(int mode, int idx) {
    if (mode == 1) {
        return idx == 0 ? vec2(-0.32, 0.02) : vec2(0.32, -0.02);
    }
    if (mode == 2) {
        if (idx == 0) return vec2(0.0, 0.0);
        if (idx == 1) return vec2(-0.24, -0.18);
        return vec2(0.24, 0.18);
    }
    return vec2(0.0);
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    vec2 p = uv * 2.0 - 1.0;
    p.x *= iResolution.x / iResolution.y;

    float speedNorm = clamp(iSpeed / 2.8, 0.0, 1.0);
    float intensityNorm = clamp(iIntensity * 0.01, 0.0, 1.0);
    float ringNorm = clamp(iRingCount * 0.01, 0.0, 1.0);
    float decayNorm = clamp(iDecay * 0.01, 0.0, 1.0);

    float bass = clamp(iAudioBass, 0.0, 1.0);
    float beat = clamp(iAudioBeatPulse, 0.0, 1.0);
    float level = clamp(iAudioLevel, 0.0, 1.0);
    float flux = clamp(iAudioSpectralFlux, 0.0, 1.0);

    float t = iTime * (0.40 + iSpeed * 0.90);

    // Fallback pulse keeps motion alive when audio features are near zero.
    float fallbackBeat = pow(max(0.0, sin(t * (1.75 + speedNorm * 1.85))), 6.0);
    float fallbackSwell = 0.5 + 0.5 * sin(t * 0.55 + sin(t * 0.17) * 2.1);
    float fallbackEnergy = 0.18 + fallbackBeat * 0.82 + fallbackSwell * 0.24;

    float audioEnergy = max(level, max(bass * 0.95 + flux * 0.35, beat));
    float audioPresence = smoothstep(0.02, 0.14, level + bass + beat + flux);
    float pulse = mix(0.25 + fallbackBeat * 0.75, max(beat, fallbackBeat * 0.45 + 0.20), audioPresence);
    float energy = mix(fallbackEnergy, audioEnergy, audioPresence);
    energy = clamp(max(energy, 0.25 + fallbackBeat * 0.55), 0.0, 1.4);

    int sceneMode = clamp(iScene, 0, 2);
    int emitters = sceneEmitterCount(sceneMode);

    float ringTarget = floor(mix(3.0, 14.0, ringNorm));
    float ringsPerEmitter = max(2.0, ringTarget / float(emitters));
    float decayRate = mix(1.6, 8.2, decayNorm);
    float ringSpeed = mix(0.26, 0.92, speedNorm);
    float baseWidth = mix(0.017, 0.0065, ringNorm) * mix(1.0, 0.72, pulse);

    vec3 col = vec3(0.0);

    for (int e = 0; e < 3; e++) {
        if (e >= emitters) break;
        vec2 origin = emitterPos(sceneMode, e);
        vec2 q = p - origin;
        float r = length(q);
        float a = atan(q.y, q.x);

        for (int i = 0; i < 16; i++) {
            float fi = float(i);
            if (fi >= ringsPerEmitter) break;

            float phase = fract(t * ringSpeed + float(e) * 0.21 + fi / ringsPerEmitter);
            float maxRadius = sceneMode == 0 ? 1.45 : 1.20;
            float radius = phase * maxRadius;

            float radialJitter = hash21(vec2(fi + float(e) * 37.0, floor(t * 0.75 + float(e) * 4.0))) - 0.5;
            radius += radialJitter * (0.010 + energy * 0.014);

            float dist = abs(r - radius);
            float ring = ringProfile(dist, baseWidth);

            float life = exp(-phase * decayRate);
            float segmentation = 0.74 + 0.26 * smoothstep(
                0.42,
                0.98,
                0.5 + 0.5 * sin(a * (8.0 + float(e) * 2.0) + fi * 1.55 - t * 2.3)
            );
            float strike = smoothstep(0.11, 0.0, phase) * (0.55 + pulse * 1.25);

            ring *= life * segmentation;
            ring *= 1.0 + strike;

            float tone = fract(fi / max(ringsPerEmitter, 1.0) + float(e) * 0.16 + t * 0.028);
            vec3 ringColor = paletteColor(tone, iPalette);
            col += ringColor * ring;
        }

        float spokeCount = mix(10.0, 26.0, ringNorm);
        float spoke = smoothstep(
            0.90,
            1.0,
            abs(sin(a * spokeCount + t * (3.0 + speedNorm * 2.8) + float(e) * 1.4))
        );
        spoke *= exp(-r * (2.8 + decayNorm * 2.6));
        spoke *= 0.16 + pulse * 0.55 + energy * 0.22;

        col += paletteColor(0.62 + a / 6.28318 + float(e) * 0.1, iPalette) * spoke;
    }

    float centerR = length(p);
    float core = exp(-centerR * (17.0 - pulse * 6.5));
    float coreRing = ringProfile(abs(centerR - (0.04 + pulse * 0.055)), 0.010);
    col += paletteColor(0.09 + t * 0.045, iPalette) * (core * (0.45 + energy * 0.7) + coreRing * 0.7);

    if (sceneMode == 1) {
        float lane = abs(p.y + sin(t * 1.6) * 0.03);
        float bridge = smoothstep(0.045, 0.008, lane) * exp(-abs(p.x) * 1.7);
        col += paletteColor(0.82 + p.x * 0.16 - t * 0.02, iPalette) * bridge * (0.18 + pulse * 0.55);
    } else if (sceneMode == 2) {
        vec2 gp = abs(fract((p + vec2(t * 0.04, -t * 0.03)) * (6.0 + ringNorm * 7.0)) - 0.5);
        float grid = smoothstep(0.49, 0.44, max(gp.x, gp.y));
        float gate = exp(-centerR * 1.9);
        col += paletteColor(0.44 + (p.x + p.y) * 0.12, iPalette) * grid * gate * (0.12 + pulse * 0.32);
    }

    col *= (0.34 + intensityNorm * 1.75) * (0.78 + energy * 0.42);

    float vignette = smoothstep(1.45, 0.14, length(p));
    col *= 0.35 + vignette * 0.75;

    col = col / (1.0 + col * 0.55);
    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
