#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iSpeed;
uniform float iDensity;
uniform float iTrailLength;
uniform float iGlow;
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

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
    if (id == 1) return iqPalette(t, vec3(0.5, 0.2, 0.0), vec3(0.5, 0.4, 0.2), vec3(1.0, 0.7, 0.4), vec3(0.0, 0.15, 0.2));
    if (id == 2) return iqPalette(t, vec3(0.5, 0.6, 0.8), vec3(0.2, 0.3, 0.2), vec3(0.6, 0.8, 1.0), vec3(0.0, 0.1, 0.3));
    if (id == 3) return iqPalette(t, vec3(0.2, 0.5, 0.4), vec3(0.3, 0.4, 0.4), vec3(0.8, 0.7, 0.9), vec3(0.6, 0.3, 0.7));
    if (id == 4) return iqPalette(t, vec3(0.5, 0.2, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 1.0), vec3(0.8, 0.1, 0.6));
    return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
}

// Single meteor trail
float meteor(vec2 uv, vec2 origin, float angle, float progress, float trailLen) {
    // Rotate UV to meteor's frame
    float ca = cos(angle), sa = sin(angle);
    mat2 rot = mat2(ca, sa, -sa, ca);
    vec2 local = rot * (uv - origin);

    // Meteor head position
    float headX = progress * 2.0;
    local.x -= headX;

    // Trail shape: thin line behind the head
    float trail = smoothstep(0.0, trailLen, -local.x) * smoothstep(trailLen + 0.1, trailLen, -local.x);
    float width = 0.004 + 0.002 * smoothstep(0.0, trailLen * 0.3, -local.x);
    float shape = smoothstep(width, width * 0.3, abs(local.y)) * trail;

    // Head glow
    float headDist = length(local);
    float head = exp(-headDist * headDist * 800.0);

    return shape + head * 2.0;
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    vec2 aspect = vec2(iResolution.x / iResolution.y, 1.0);
    uv = (uv - 0.5) * aspect;

    float time = iTime * iSpeed * 0.3;

    // Starfield background
    vec3 col = vec3(0.0);
    vec2 starUV = (uv + 0.5) * 150.0;
    vec2 starCell = floor(starUV);
    float starRng = hash21(starCell);
    if (starRng > 0.95) {
        vec2 starLocal = fract(starUV) - 0.5;
        float starDist = length(starLocal);
        float twinkle = 0.5 + 0.5 * sin(iTime * 2.0 + starRng * 50.0);
        col += vec3(0.6, 0.7, 1.0) * smoothstep(0.04, 0.0, starDist) * twinkle * 0.5;
    }

    // Meteors
    float trailLen = 0.1 + iTrailLength * 0.004;
    int meteorCount = 4 + int(iDensity * 0.12);

    for (int i = 0; i < 16; i++) {
        if (i >= meteorCount) break;
        float fi = float(i);

        // Deterministic meteor parameters per cycle
        float cycle = floor(time * 0.3 + fi * 0.618);
        float seed = hash11(fi * 17.0 + cycle * 31.0);

        // Origin: top-right region
        vec2 origin = vec2(
            0.3 + seed * 0.6,
            0.2 + hash11(seed * 100.0 + cycle) * 0.5
        ) * aspect - aspect * 0.5;

        // Angle: mostly upper-left to lower-right
        float angle = -0.6 - seed * 0.4;

        // Progress through the streak
        float progress = fract(time * (0.3 + seed * 0.4) + fi * 0.618);

        // Visibility window
        float visible = smoothstep(0.0, 0.05, progress) * smoothstep(0.8, 0.6, progress);

        float m = meteor(uv, origin, angle, progress, trailLen) * visible;

        // Color: hot white head → palette tail
        float headAmount = smoothstep(0.5, 2.0, m);
        vec3 tailColor = paletteColor(seed + fi * 0.1, iPalette);
        vec3 headColor = vec3(1.0, 0.95, 0.85);
        vec3 meteorColor = mix(tailColor, headColor, headAmount);

        col += meteorColor * m * iGlow * 0.015;
    }

    // Atmospheric glow at horizon
    float horizon = exp(-abs(uv.y + 0.3) * 8.0) * 0.05;
    col += paletteColor(0.5, iPalette) * horizon;

    col = col / (1.0 + col * 0.4);
    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
