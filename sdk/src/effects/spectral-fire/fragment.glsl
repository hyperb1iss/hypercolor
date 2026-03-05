#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iAudioBass;
uniform float iAudioMid;
uniform float iAudioTreble;
uniform float iAudioLevel;
uniform float iAudioBeatPulse;
uniform float iAudioSpectralFlux;

uniform float iSpeed;
uniform float iFlameHeight;
uniform float iTurbulence;
uniform float iIntensity;
uniform int iPalette;

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

float noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    f = f * f * (3.0 - 2.0 * f);

    float a = hash21(i);
    float b = hash21(i + vec2(1.0, 0.0));
    float c = hash21(i + vec2(0.0, 1.0));
    float d = hash21(i + vec2(1.0, 1.0));

    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

float fbm(vec2 p) {
    float value = 0.0;
    float amplitude = 0.5;
    for (int i = 0; i < 5; i++) {
        value += amplitude * noise(p);
        p *= 2.0;
        amplitude *= 0.5;
    }
    return value;
}

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
    // Fire: black → red → orange → yellow
    if (id == 1) return iqPalette(t, vec3(0.5, 0.2, 0.0), vec3(0.5, 0.4, 0.2), vec3(1.0, 0.7, 0.4), vec3(0.0, 0.15, 0.2));
    if (id == 2) return iqPalette(t, vec3(0.5, 0.6, 0.8), vec3(0.2, 0.3, 0.2), vec3(0.6, 0.8, 1.0), vec3(0.0, 0.1, 0.3));
    if (id == 3) return iqPalette(t, vec3(0.2, 0.5, 0.4), vec3(0.3, 0.4, 0.4), vec3(0.8, 0.7, 0.9), vec3(0.6, 0.3, 0.7));
    if (id == 4) return iqPalette(t, vec3(0.5, 0.2, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 1.0), vec3(0.8, 0.1, 0.6));
    return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
}

// Blackbody approximation for fire temperature
vec3 blackbody(float t) {
    t = clamp(t, 0.0, 1.0);
    vec3 col = vec3(0.0);
    // Red channel ramps first
    col.r = smoothstep(0.0, 0.4, t);
    // Green follows
    col.g = smoothstep(0.2, 0.7, t) * 0.9;
    // Blue last (only at very hot)
    col.b = smoothstep(0.6, 1.0, t) * 0.6;
    return col;
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    float aspect = iResolution.x / iResolution.y;

    float time = iTime * iSpeed * 0.4;

    // Map audio to flame columns
    // Divide into frequency bands across X axis
    float bass = iAudioBass;
    float mid = iAudioMid;
    float treble = iAudioTreble;
    float level = iAudioLevel;

    // Per-column audio intensity
    float x = uv.x;
    float bandIntensity;
    if (x < 0.33) {
        bandIntensity = bass;
    } else if (x < 0.66) {
        bandIntensity = mid;
    } else {
        bandIntensity = treble;
    }
    // Smooth blend between bands
    float blendBass = smoothstep(0.33, 0.0, x) * bass;
    float blendMid = (1.0 - abs(x - 0.5) * 3.0) * mid;
    float blendTreble = smoothstep(0.66, 1.0, x) * treble;
    bandIntensity = blendBass + blendMid + blendTreble;

    // Flame height driven by audio
    float baseHeight = iFlameHeight * 0.008;
    float flameHeight = baseHeight + bandIntensity * 0.4;

    // Turbulent flame shape
    float turbScale = 3.0 + iTurbulence * 0.05;
    float turb1 = fbm(vec2(x * turbScale, time * 2.0));
    float turb2 = fbm(vec2(x * turbScale * 2.0 + 10.0, time * 3.0));

    // Flame density: strongest at bottom, tapers up
    float flameBase = 1.0 - uv.y;
    float flameTaper = smoothstep(flameHeight, 0.0, 1.0 - uv.y);

    // Add turbulent displacement
    float displaced = flameBase + (turb1 - 0.5) * 0.3 + (turb2 - 0.5) * 0.15;
    float flame = smoothstep(0.0, flameHeight, displaced) * flameTaper;

    // Temperature map: hotter at base, cooler at tips
    float temperature = flame * (1.0 - (1.0 - uv.y) * 0.5);
    temperature += bandIntensity * 0.3;

    // Color: mix blackbody with palette
    vec3 fireColor = blackbody(temperature * 0.8);
    vec3 palColor = paletteColor(temperature * 0.5 + x * 0.2, iPalette);
    vec3 col = mix(fireColor, palColor, 0.3) * flame * iIntensity * 0.02;

    // Bright core
    float core = smoothstep(0.2, 0.0, 1.0 - uv.y) * flame * 0.5;
    col += vec3(1.0, 0.9, 0.6) * core;

    // Sparks on beat
    float sparkSeed = floor(time * 4.0 + x * 10.0);
    float sparkRng = hash21(vec2(sparkSeed, floor(uv.y * 20.0)));
    if (sparkRng > 0.95 && iAudioBeatPulse > 0.3) {
        float sparkY = fract(time * 2.0 + sparkRng * 5.0);
        if (abs(uv.y - sparkY) < 0.01 && uv.y > 0.3) {
            col += paletteColor(sparkRng, iPalette) * 0.5;
        }
    }

    // Ember glow at base
    float emberGlow = exp(-(1.0 - uv.y) * 3.0) * level * 0.3;
    col += paletteColor(0.1, iPalette) * emberGlow;

    col = col / (1.0 + col * 0.3);
    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
