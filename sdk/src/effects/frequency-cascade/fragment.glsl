#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

// Audio
uniform float iAudioLevel;
uniform float iAudioBass;
uniform float iAudioMid;
uniform float iAudioTreble;
uniform float iAudioBeat;
uniform float iAudioBeatPulse;
uniform float iAudioSpectralFlux;
uniform float iAudioBrightness;
uniform float iAudioSwell;
uniform vec3 iAudioFluxBands;

// Controls
uniform float iSpeed;
uniform float iIntensity;
uniform float iSmoothing;
uniform float iBarWidth;
uniform int iPalette;
uniform float iGlow;

// ── Noise ──────────────────────────────────────────────────────────────

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// ── Palettes ───────────────────────────────────────────────────────────

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
    if (id == 1) return iqPalette(t, vec3(0.2, 0.5, 0.4), vec3(0.3, 0.4, 0.4), vec3(0.8, 0.7, 0.9), vec3(0.6, 0.3, 0.7));
    if (id == 2) return iqPalette(t, vec3(0.5, 0.2, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 1.0), vec3(0.8, 0.1, 0.6));
    if (id == 3) return iqPalette(t, vec3(0.5, 0.2, 0.0), vec3(0.5, 0.4, 0.2), vec3(1.0, 0.7, 0.4), vec3(0.0, 0.15, 0.2));
    if (id == 4) return iqPalette(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 1.0), vec3(0.0, 0.1, 0.2));
    if (id == 5) return iqPalette(t, vec3(0.5, 0.6, 0.8), vec3(0.2, 0.3, 0.2), vec3(0.6, 0.8, 1.0), vec3(0.0, 0.1, 0.3));
    return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
}

// ── Waterfall ──────────────────────────────────────────────────────────

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    float time = iTime * iSpeed * 0.5;

    // Frequency position (x-axis = frequency, logarithmic feel)
    float freqPos = uv.x;

    // Simulate mel-band energy for this horizontal position
    // Map across bass → mid → treble
    float bandEnergy;
    if (freqPos < 0.33) {
        bandEnergy = iAudioBass * (1.0 + iAudioFluxBands.x * 0.5);
    } else if (freqPos < 0.66) {
        bandEnergy = iAudioMid * (1.0 + iAudioFluxBands.y * 0.5);
    } else {
        bandEnergy = iAudioTreble * (1.0 + iAudioFluxBands.z * 0.5);
    }

    // Cross-fade between bands for smooth transitions
    float bandMix = fract(freqPos * 3.0);
    float smoothBand = smoothstep(0.0, 1.0, bandMix);

    float bassContrib = iAudioBass * smoothstep(0.33, 0.0, freqPos);
    float midContrib = iAudioMid * (1.0 - abs(freqPos - 0.5) * 2.5);
    float trebleContrib = iAudioTreble * smoothstep(0.66, 1.0, freqPos);
    bandEnergy = bassContrib + midContrib + trebleContrib;

    // Add spectral flux for liveliness
    bandEnergy += iAudioSpectralFlux * 0.3;

    // Vertical waterfall: history scrolls downward
    // Current energy at the top, fading history below
    float historyPos = 1.0 - uv.y; // 0 at top, 1 at bottom
    float decay = exp(-historyPos * (3.0 - iSmoothing * 0.025));
    float energy = bandEnergy * decay;

    // Add noise for texture variation in the waterfall
    float noiseVal = hash21(vec2(floor(freqPos * 64.0), floor(uv.y * 200.0 + time * 30.0)));
    energy += noiseVal * 0.05 * energy;

    // Bar quantization
    float barCount = 32.0 + iBarWidth * 0.6;
    float barPos = floor(freqPos * barCount) / barCount;
    float barFract = fract(freqPos * barCount);
    float barGap = smoothstep(0.0, 0.08, barFract) * smoothstep(1.0, 0.92, barFract);

    // Color: frequency position maps to palette, energy controls brightness
    float colorT = barPos + time * 0.03;
    vec3 col = paletteColor(colorT, iPalette);

    // Intensity modulation
    float brightness = energy * iIntensity * 0.015 * barGap;

    // Beat pulse — flash on transients
    float beatFlash = iAudioBeatPulse * 0.4 * decay;
    brightness += beatFlash;

    // Glow: soft bloom around bright areas
    float glowRadius = iGlow * 0.003;
    float glow = 0.0;
    for (int i = -2; i <= 2; i++) {
        float offset = float(i) * glowRadius;
        float sampleEnergy = bandEnergy * exp(-(historyPos + offset * 2.0) * (3.0 - iSmoothing * 0.025));
        glow += sampleEnergy * exp(-abs(offset) * 50.0);
    }
    glow *= 0.1 * iGlow * 0.01;

    col *= brightness;
    col += paletteColor(colorT + 0.1, iPalette) * glow;

    // Subtle scan line for depth
    float scanline = 0.95 + 0.05 * sin(uv.y * iResolution.y * 3.14159);
    col *= scanline;

    // Top edge highlight (current audio)
    float topEdge = smoothstep(0.05, 0.0, historyPos) * bandEnergy * 2.0;
    col += paletteColor(freqPos, iPalette) * topEdge * iIntensity * 0.01;

    // Tonemapping
    col = col / (1.0 + col * 0.5);

    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
