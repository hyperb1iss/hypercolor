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

void main() {
    vec2 uv = (gl_FragCoord.xy - 0.5 * iResolution) / iResolution.y;
    float time = iTime * iSpeed * 0.3;

    float r = length(uv);
    float angle = atan(uv.y, uv.x);

    float bass = iAudioBass;
    float beatPulse = iAudioBeatPulse;
    float level = iAudioLevel;

    vec3 col = vec3(0.0);

    // Expanding shockwave rings on beat
    float ringCount = 3.0 + iRingCount * 0.09;
    float decayRate = 2.0 + iDecay * 0.06;

    for (int i = 0; i < 12; i++) {
        if (float(i) >= ringCount) break;
        float fi = float(i);

        // Each ring expands outward from center
        float ringPhase = fract(time * 0.2 + fi / ringCount);
        float ringRadius = ringPhase * 1.2;

        // Ring fades as it expands
        float ringFade = exp(-ringPhase * decayRate);

        // Ring width modulated by bass
        float ringWidth = 0.015 + bass * 0.02 + beatPulse * 0.01;

        // Distance from this pixel to the ring
        float ringDist = abs(r - ringRadius);
        float ring = smoothstep(ringWidth, ringWidth * 0.2, ringDist) * ringFade;

        // Angular variation — ring isn't perfectly circular
        float angularWarp = sin(angle * 3.0 + fi * 2.0 + time) * 0.02 * bass;
        ring *= 1.0 + angularWarp;

        // Beat pulse makes the newest ring flash bright
        float isBeatRing = smoothstep(0.1, 0.0, ringPhase);
        ring += isBeatRing * beatPulse * 0.5 * smoothstep(ringWidth * 2.0, 0.0, ringDist);

        // Color shifts per ring
        float colorT = fi / ringCount + ringPhase * 0.3;
        vec3 ringColor = paletteColor(colorT, iPalette);

        col += ringColor * ring * iIntensity * 0.015;
    }

    // Central glow — pulses with bass
    float centerGlow = exp(-r * (4.0 - bass * 2.0)) * (0.3 + bass * 0.5 + beatPulse * 0.3);
    col += paletteColor(time * 0.05, iPalette) * centerGlow * iIntensity * 0.01;

    // Radial displacement lines on beat
    float radialLines = sin(angle * 8.0 + time * 2.0) * 0.5 + 0.5;
    radialLines *= exp(-r * 3.0) * beatPulse * 0.3;
    col += paletteColor(angle / 6.28 + 0.5, iPalette) * radialLines;

    // Particle burst on strong beats
    float particleSeed = floor(time * 4.0);
    for (int p = 0; p < 8; p++) {
        float fp = float(p);
        float pAngle = hash21(vec2(fp, particleSeed)) * 6.28318;
        float pSpeed = 0.3 + hash21(vec2(fp + 10.0, particleSeed)) * 0.7;
        float pPhase = fract(time * 0.5 - particleSeed * 0.25);
        float pRadius = pPhase * pSpeed;
        float pFade = exp(-pPhase * 3.0) * beatPulse;

        vec2 pPos = vec2(cos(pAngle), sin(pAngle)) * pRadius;
        float pDist = length(uv - pPos);
        float particle = smoothstep(0.02, 0.005, pDist) * pFade;

        col += paletteColor(fp * 0.125, iPalette) * particle * 0.5;
    }

    col = col / (1.0 + col * 0.5);
    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
