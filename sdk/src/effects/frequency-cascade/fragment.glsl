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
uniform float iAudioBeatPulse;
uniform float iAudioOnsetPulse;
uniform float iAudioSpectralFlux;
uniform float iAudioHarmonicHue;
uniform float iAudioSwell;
uniform vec3 iAudioFluxBands;

// Controls
uniform float iSpeed;
uniform float iIntensity;
uniform float iSmoothing;
uniform float iBarWidth;
uniform int iPalette;
uniform float iGlow;
uniform int iScene;

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return iqPalette(t, vec3(0.50, 0.28, 0.54), vec3(0.48, 0.45, 0.45), vec3(1.00, 0.85, 0.70), vec3(0.88, 0.18, 0.52));
    if (id == 1) return iqPalette(t, vec3(0.18, 0.50, 0.40), vec3(0.35, 0.40, 0.45), vec3(0.75, 0.70, 0.85), vec3(0.62, 0.30, 0.72));
    if (id == 2) return iqPalette(t, vec3(0.52, 0.18, 0.48), vec3(0.52, 0.45, 0.50), vec3(1.00, 1.00, 1.00), vec3(0.84, 0.10, 0.60));
    if (id == 3) return iqPalette(t, vec3(0.50, 0.22, 0.02), vec3(0.50, 0.40, 0.20), vec3(1.00, 0.72, 0.38), vec3(0.02, 0.16, 0.24));
    if (id == 4) return iqPalette(t, vec3(0.54, 0.38, 0.26), vec3(0.44, 0.32, 0.30), vec3(0.90, 0.75, 0.62), vec3(0.06, 0.22, 0.38));
    if (id == 5) return iqPalette(t, vec3(0.52, 0.60, 0.78), vec3(0.22, 0.30, 0.22), vec3(0.62, 0.82, 1.00), vec3(0.00, 0.10, 0.32));
    return iqPalette(t, vec3(0.50, 0.28, 0.54), vec3(0.48, 0.45, 0.45), vec3(1.00, 0.85, 0.70), vec3(0.88, 0.18, 0.52));
}

float audioPresence() {
    float fluxMean = dot(iAudioFluxBands, vec3(0.3333));
    float signal =
        iAudioLevel * 1.15 +
        iAudioBeatPulse * 0.95 +
        iAudioOnsetPulse * 0.65 +
        iAudioSpectralFlux * 0.70 +
        fluxMean * 0.55;
    return smoothstep(0.035, 0.16, signal);
}

float audioEnergy(float freq, float barId, float time) {
    float bassWeight = 1.0 - smoothstep(0.05, 0.92, freq);
    float trebleWeight = smoothstep(0.25, 0.98, freq);
    float midWeight = exp(-pow((freq - 0.50) * 2.8, 2.0));

    float lowMid = mix(iAudioBass, iAudioMid, smoothstep(0.10, 0.55, freq));
    float midHigh = mix(iAudioMid, iAudioTreble, smoothstep(0.40, 0.92, freq));
    float stitched = mix(lowMid, midHigh, smoothstep(0.35, 0.70, freq));

    float flux = mix(iAudioFluxBands.x, iAudioFluxBands.y, smoothstep(0.05, 0.55, freq));
    flux = mix(flux, iAudioFluxBands.z, smoothstep(0.45, 0.98, freq));

    float sparkleSeed = hash21(vec2(barId * 0.123, floor(time * 12.0)));
    float sparkle = iAudioSpectralFlux * (0.20 + 0.80 * sparkleSeed);

    float energy =
        stitched * 0.82 +
        iAudioBass * bassWeight * 0.30 +
        iAudioTreble * trebleWeight * 0.32 +
        iAudioMid * midWeight * 0.42 +
        flux * 0.55 +
        sparkle +
        iAudioSwell * 0.18;

    return clamp(energy, 0.0, 1.6);
}

float fallbackEnergy(float barId, float freq, float time, int scene) {
    float n = hash21(vec2(barId, floor(time * 2.4)));
    float waveA = 0.5 + 0.5 * sin(time * 1.65 + barId * 0.24 + n * 6.28318);
    float waveB = 0.5 + 0.5 * sin(time * 0.86 - barId * 0.19 + sin(time * 0.44 + barId * 0.08));
    float pulse = pow(max(0.0, sin(time * (2.0 + float(scene) * 0.22) + barId * (0.10 + float(scene) * 0.01))), 2.4);

    float contour = 0.35 + 0.65 * (1.0 - abs(freq - 0.5) * 1.35);
    if (scene == 2) contour = 0.45 + 0.55 * smoothstep(0.0, 1.0, freq);
    if (scene == 3) contour = 0.45 + 0.55 * (1.0 - smoothstep(0.0, 1.0, freq));

    float energy = mix(waveA, waveB, 0.45) * (0.64 + pulse * 0.72) + contour * 0.20;
    return clamp(energy, 0.0, 1.35);
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    float aspect = iResolution.x / max(iResolution.y, 1.0);

    float speed = max(iSpeed, 0.05);
    float time = iTime * (0.28 + speed * 0.44);
    float intensity = clamp(iIntensity * 0.01, 0.0, 1.0);
    float smoothness = clamp(iSmoothing * 0.01, 0.0, 1.0);
    float glowAmount = clamp(iGlow * 0.01, 0.0, 1.0);

    float barCount = mix(140.0, 20.0, clamp(iBarWidth * 0.01, 0.0, 1.0));
    float barPos = uv.x * barCount;
    float barId = floor(barPos);
    float barCell = fract(barPos);

    float barMask = smoothstep(0.02, 0.11, barCell) * smoothstep(0.02, 0.11, 1.0 - barCell);

    float freq = barId / max(barCount - 1.0, 1.0);
    float warpedFreq = freq;
    if (iScene == 1) warpedFreq = pow(freq, 0.84);
    if (iScene == 2) warpedFreq = abs(freq * 2.0 - 1.0);
    if (iScene == 3) warpedFreq = fract(freq + 0.12 * sin(time * 0.38));

    float audioActive = audioPresence();
    float reactiveEnergy = audioEnergy(warpedFreq, barId, time);
    float proceduralEnergy = fallbackEnergy(barId, warpedFreq, time, iScene);

    float energy = mix(proceduralEnergy, reactiveEnergy, audioActive);
    energy += iAudioBeatPulse * 0.25 * audioActive;

    if (iScene == 1) {
        energy *= 0.82 + 0.18 * sin(time * 5.6 + uv.y * 24.0 + barId * 0.14);
    } else if (iScene == 2) {
        float center = exp(-abs(uv.x - 0.5) * 7.0);
        energy *= 0.74 + center * 0.56;
    } else if (iScene == 3) {
        float chop = 0.65 + 0.35 * step(0.56, fract(time * 1.05 + barId * 0.039));
        energy *= chop;
    }

    energy = clamp(energy, 0.0, 1.8);

    float barHeight = clamp(
        mix(0.08, 0.02, audioActive) + energy * mix(0.36, 1.28, intensity),
        0.02,
        1.0
    );

    float barBody = 1.0 - smoothstep(barHeight - 0.010, barHeight + 0.010, uv.y);
    float ledRows = mix(20.0, 8.0, smoothness);
    float ledCell = fract(uv.y * ledRows - time * 0.22);
    float ledMask = smoothstep(0.05, 0.20, ledCell) * smoothstep(0.05, 0.20, 1.0 - ledCell);
    barBody *= mix(0.78, 1.0, ledMask);

    float historyY = 1.0 - uv.y;
    float trailDecay = mix(9.0, 2.4, smoothness);
    float trailScan = 0.55 + 0.45 * sin(historyY * 72.0 + time * (4.2 + speed * 1.3) + barId * 0.35);
    float waterfall = exp(-historyY * trailDecay) * trailScan * energy;
    waterfall *= smoothstep(0.16, 0.96, uv.y);
    waterfall *= 0.55 + 0.45 * barMask;

    float rim = exp(-abs(uv.y - barHeight) * mix(115.0, 44.0, glowAmount));
    float beam = exp(-abs(barCell - 0.5) * mix(28.0, 8.0, glowAmount));
    float bloom = (rim + waterfall * 0.62 + barBody * 0.20) * beam * (0.22 + glowAmount * 1.30);

    float paletteT = warpedFreq + time * (0.05 + float(iScene) * 0.008);
    vec3 baseColor = paletteColor(paletteT, iPalette);
    vec3 accentColor = paletteColor(paletteT + 0.17 + iAudioHarmonicHue * 0.0015, iPalette);

    vec3 bgLow = paletteColor(0.08 + time * 0.01, iPalette) * 0.03;
    vec3 bgHigh = paletteColor(0.46 + time * 0.01, iPalette) * 0.12;
    vec3 color = mix(bgLow, bgHigh, pow(uv.y, 1.3));

    float bodyLight = barBody * (0.34 + energy * 1.45);
    color += baseColor * bodyLight * barMask * (0.30 + intensity * 1.75);
    color += accentColor * rim * (0.16 + intensity * 1.10);
    color += mix(baseColor, accentColor, 0.62) * waterfall * (0.12 + intensity * 0.86);
    color += accentColor * bloom;

    if (iScene == 1) {
        float gridWave = abs(fract(uv.y * 17.0 - time * 1.3) - 0.5) - 0.44;
        float grid = 1.0 - smoothstep(0.0, 0.02, gridWave);
        color += accentColor * grid * 0.10 * (0.35 + energy);
    } else if (iScene == 2) {
        float tunnel = exp(-abs(uv.x - 0.5) * 8.0) * (0.5 + 0.5 * sin(uv.y * 30.0 - time * 3.0));
        color += mix(baseColor, accentColor, 0.5) * tunnel * 0.14;
    } else if (iScene == 3) {
        float prism = pow(abs(sin((uv.x - 0.5) * aspect * 18.0 + uv.y * 7.0 + time * 2.9)), 18.0);
        color += accentColor * prism * 0.16;
    }

    float scanline = 0.95 + 0.05 * sin(gl_FragCoord.y * 1.8 + time * 2.0);
    color *= scanline;

    vec2 centered = (uv - 0.5) * vec2(aspect, 1.0);
    float vignette = 1.0 - smoothstep(0.28, 1.25, length(centered));
    color *= vignette;

    color = color / (1.0 + color * 0.34);
    color = pow(max(color, 0.0), vec3(0.95));

    fragColor = vec4(clamp(color, 0.0, 1.0), 1.0);
}
