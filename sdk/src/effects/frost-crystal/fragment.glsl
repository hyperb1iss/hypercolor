#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iSpeed;
uniform float iScale;
uniform float iEdgeGlow;
uniform float iGrowth;
uniform int iPalette;

// Hash
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

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
    if (id == 1) return iqPalette(t, vec3(0.5, 0.6, 0.8), vec3(0.2, 0.3, 0.2), vec3(0.6, 0.8, 1.0), vec3(0.0, 0.1, 0.3));
    if (id == 2) return iqPalette(t, vec3(0.6, 0.7, 0.9), vec3(0.1, 0.15, 0.1), vec3(0.5, 0.7, 1.0), vec3(0.1, 0.15, 0.25));
    if (id == 3) return iqPalette(t, vec3(0.2, 0.5, 0.4), vec3(0.3, 0.4, 0.4), vec3(0.8, 0.7, 0.9), vec3(0.6, 0.3, 0.7));
    if (id == 4) return iqPalette(t, vec3(0.5, 0.2, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 1.0), vec3(0.8, 0.1, 0.6));
    return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
}

// Worley noise — returns (F1, F2) distances
vec2 worley(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);

    float d1 = 1.0;
    float d2 = 1.0;

    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            vec2 neighbor = vec2(float(x), float(y));
            vec2 point = hash22(i + neighbor);
            // Animate points slowly
            point = 0.5 + 0.5 * sin(iTime * iSpeed * 0.2 + 6.28318 * point);
            vec2 diff = neighbor + point - f;
            float dist = length(diff);

            if (dist < d1) {
                d2 = d1;
                d1 = dist;
            } else if (dist < d2) {
                d2 = dist;
            }
        }
    }

    return vec2(d1, d2);
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    vec2 aspect = vec2(iResolution.x / iResolution.y, 1.0);
    vec2 p = (uv - 0.5) * aspect;

    float time = iTime * iSpeed * 0.15;

    float scale = 3.0 + iScale * 0.07;

    // Multi-scale Worley for crystalline structure
    vec2 w1 = worley(p * scale);
    vec2 w2 = worley(p * scale * 2.0 + vec2(5.0, 3.0));
    vec2 w3 = worley(p * scale * 0.5 + vec2(-2.0, 8.0));

    // Edge detection: F2-F1 gives cell edges
    float edge1 = w1.y - w1.x;
    float edge2 = w2.y - w2.x;
    float edge3 = w3.y - w3.x;

    // Crystal edges glow
    float edgeIntensity = iEdgeGlow * 0.01;
    float edges = smoothstep(0.15, 0.02, edge1) * 0.6
                + smoothstep(0.12, 0.01, edge2) * 0.3
                + smoothstep(0.2, 0.05, edge3) * 0.2;
    edges *= edgeIntensity;

    // Growth animation: crystals expand from center outward
    float growthRadius = iGrowth * 0.01 * (0.8 + 0.3 * sin(time * 0.3));
    float fromCenter = length(p);
    float growthMask = smoothstep(growthRadius + 0.3, growthRadius - 0.1, fromCenter);

    // Cell interior fill — subtle facets
    float cellFill = smoothstep(0.0, 0.3, w1.x) * 0.15 * growthMask;

    // Frost crystalline color
    float colorT = w1.x * 2.0 + edge1 * 3.0 + time * 0.05;
    vec3 edgeColor = paletteColor(colorT, iPalette);
    vec3 fillColor = paletteColor(colorT + 0.3, iPalette) * 0.3;

    vec3 col = edgeColor * edges * growthMask + fillColor * cellFill;

    // Sparkle points at cell centers
    float sparkle = smoothstep(0.08, 0.0, w1.x);
    float twinkle = 0.5 + 0.5 * sin(iTime * 4.0 + hash21(floor(p * scale)) * 30.0);
    col += vec3(0.8, 0.9, 1.0) * sparkle * twinkle * 0.4 * growthMask;

    // Background frost haze
    float haze = smoothstep(0.6, 0.0, fromCenter) * 0.03;
    col += paletteColor(0.7, iPalette) * haze;

    col = col / (1.0 + col * 0.4);
    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
