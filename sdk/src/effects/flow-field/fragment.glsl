#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iSpeed;
uniform float iParticles;
uniform float iTrailFade;
uniform float iNoiseScale;
uniform int iPalette;

float hash11(float p) {
    p = fract(p * 0.1031);
    p *= p + 33.33;
    p *= p + p;
    return fract(p);
}

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// Simplex-like noise for flow field
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

// Flow field angle at a point
float flowAngle(vec2 p, float time) {
    float scale = 1.0 + iNoiseScale * 0.04;
    float n1 = noise(p * scale + time * 0.2);
    float n2 = noise(p * scale * 1.5 + vec2(5.0, 3.0) + time * 0.15);
    return (n1 + n2 * 0.5) * 6.28318;
}

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
    if (id == 1) return iqPalette(t, vec3(0.5, 0.2, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 1.0), vec3(0.8, 0.1, 0.6));
    if (id == 2) return iqPalette(t, vec3(0.2, 0.5, 0.4), vec3(0.3, 0.4, 0.4), vec3(0.8, 0.7, 0.9), vec3(0.6, 0.3, 0.7));
    if (id == 3) return iqPalette(t, vec3(0.5, 0.2, 0.0), vec3(0.5, 0.4, 0.2), vec3(1.0, 0.7, 0.4), vec3(0.0, 0.15, 0.2));
    if (id == 4) return iqPalette(t, vec3(0.5, 0.6, 0.8), vec3(0.2, 0.3, 0.2), vec3(0.6, 0.8, 1.0), vec3(0.0, 0.1, 0.3));
    return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    vec2 aspect = vec2(iResolution.x / iResolution.y, 1.0);
    vec2 p = (uv - 0.5) * aspect;

    float time = iTime * iSpeed * 0.2;

    vec3 col = vec3(0.0);

    // Simulate particles following the flow field
    int numParticles = 20 + int(iParticles * 0.8);
    float trailLen = 8.0 + iTrailFade * 0.2;

    for (int i = 0; i < 100; i++) {
        if (i >= numParticles) break;
        float fi = float(i);

        // Particle seed position (wraps around)
        float seed = hash11(fi * 7.13);
        float seed2 = hash11(fi * 13.37 + 100.0);
        vec2 particleStart = vec2(
            (seed - 0.5) * aspect.x,
            (seed2 - 0.5)
        );

        // Advect particle through flow field
        vec2 pos = particleStart;
        float stepSize = 0.02;

        // Use time to cycle particle through its trail
        float particleTime = fract(time * 0.1 + fi * 0.0618);
        int totalSteps = int(trailLen);

        for (int step = 0; step < 30; step++) {
            if (step >= totalSteps) break;
            float fStep = float(step);

            float angle = flowAngle(pos * 3.0, time);
            vec2 dir = vec2(cos(angle), sin(angle));

            // Check proximity to this trail segment
            float dist = length(p - pos);
            float trailFade = 1.0 - fStep / trailLen;
            float width = 0.003 + 0.002 * trailFade;
            float contrib = smoothstep(width, width * 0.3, dist) * trailFade;

            // Color varies along the trail
            float colorT = fi * 0.1 + fStep * 0.03 + time * 0.02;
            col += paletteColor(colorT, iPalette) * contrib * 0.15;

            // Advect
            pos += dir * stepSize;

            // Wrap around edges
            pos = mod(pos + aspect * 0.5, aspect) - aspect * 0.5;
        }
    }

    // Background flow visualization (subtle)
    float bgAngle = flowAngle(p * 3.0, time);
    float bgLines = abs(sin(bgAngle * 3.0 + length(p) * 10.0));
    col += paletteColor(bgAngle / 6.28, iPalette) * smoothstep(0.9, 1.0, bgLines) * 0.02;

    col = col / (1.0 + col * 0.4);
    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
