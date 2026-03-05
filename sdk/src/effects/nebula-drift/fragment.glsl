#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iSpeed;
uniform float iDensity;
uniform float iWarp;
uniform float iBrightness;
uniform int iPalette;

// Hash functions
float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

vec2 hash22(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * vec3(0.1031, 0.1030, 0.0973));
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.xx + p3.yz) * p3.zy);
}

// Value noise
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

// fBm with domain warping
float fbm(vec2 p, float time) {
    float value = 0.0;
    float amplitude = 0.5;
    float frequency = 1.0;
    float totalAmp = 0.0;

    for (int i = 0; i < 6; i++) {
        // Domain warp: offset each octave by noise from previous
        vec2 warpOffset = vec2(
            noise(p * 0.7 + time * 0.1 + float(i) * 1.3),
            noise(p * 0.7 + time * 0.15 + float(i) * 2.7 + 50.0)
        ) * iWarp * 0.02;

        value += amplitude * noise(p * frequency + warpOffset * frequency);
        totalAmp += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
        p += vec2(1.7, 9.2);
    }
    return value / totalAmp;
}

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
    if (id == 1) return iqPalette(t, vec3(0.2, 0.1, 0.3), vec3(0.4, 0.3, 0.5), vec3(0.8, 0.6, 1.0), vec3(0.3, 0.2, 0.5));
    if (id == 2) return iqPalette(t, vec3(0.5, 0.2, 0.0), vec3(0.5, 0.4, 0.2), vec3(1.0, 0.7, 0.4), vec3(0.0, 0.15, 0.2));
    if (id == 3) return iqPalette(t, vec3(0.2, 0.5, 0.4), vec3(0.3, 0.4, 0.4), vec3(0.8, 0.7, 0.9), vec3(0.6, 0.3, 0.7));
    if (id == 4) return iqPalette(t, vec3(0.5, 0.6, 0.8), vec3(0.2, 0.3, 0.2), vec3(0.6, 0.8, 1.0), vec3(0.0, 0.1, 0.3));
    return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    vec2 aspect = vec2(iResolution.x / iResolution.y, 1.0);
    vec2 p = (uv - 0.5) * aspect;

    float time = iTime * iSpeed * 0.15;

    // Multi-layer nebula clouds
    float scale = 2.0 + iDensity * 0.03;
    float n1 = fbm(p * scale + time * 0.3, time);
    float n2 = fbm(p * scale * 1.5 + vec2(5.0, 3.0) + time * 0.2, time * 1.3);
    float n3 = fbm(p * scale * 0.7 + vec2(-3.0, 7.0) + time * 0.1, time * 0.7);

    // Combine layers with different color channels
    float cloud1 = smoothstep(0.3, 0.7, n1);
    float cloud2 = smoothstep(0.35, 0.75, n2);
    float cloud3 = smoothstep(0.25, 0.65, n3);

    // Color mapping through palette
    vec3 col1 = paletteColor(n1 * 0.5 + time * 0.02, iPalette) * cloud1;
    vec3 col2 = paletteColor(n2 * 0.5 + 0.33 + time * 0.015, iPalette) * cloud2;
    vec3 col3 = paletteColor(n3 * 0.5 + 0.66 + time * 0.01, iPalette) * cloud3;

    vec3 nebula = col1 * 0.5 + col2 * 0.3 + col3 * 0.2;

    // Star field
    vec3 stars = vec3(0.0);
    for (int layer = 0; layer < 3; layer++) {
        float fl = float(layer);
        float starScale = 80.0 + fl * 60.0;
        vec2 starUV = (p + 0.5) * starScale;
        vec2 starCell = floor(starUV);
        vec2 starLocal = fract(starUV) - 0.5;

        float rng = hash21(starCell + fl * 100.0);
        if (rng > 0.92 - fl * 0.02) {
            vec2 offset = hash22(starCell + fl * 100.0) - 0.5;
            float d = length(starLocal - offset * 0.4);
            float twinkle = 0.6 + 0.4 * sin(iTime * (1.0 + rng * 3.0) + rng * 50.0);
            float brightness = smoothstep(0.03 - fl * 0.005, 0.0, d) * twinkle;

            // Stars dim behind dense nebula
            float obscure = 1.0 - smoothstep(0.3, 0.8, cloud1 + cloud2);
            vec3 starColor = mix(vec3(0.8, 0.9, 1.0), vec3(1.0, 0.7, 0.5), rng);
            stars += starColor * brightness * obscure * (0.3 + fl * 0.2);
        }
    }

    vec3 col = nebula * iBrightness * 0.02 + stars;

    // Subtle vignette
    float vignette = 1.0 - dot(p * 0.7, p * 0.7);
    col *= smoothstep(0.0, 0.5, vignette);

    col = col / (1.0 + col * 0.3);
    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
