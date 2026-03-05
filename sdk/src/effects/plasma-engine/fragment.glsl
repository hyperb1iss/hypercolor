#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

// Controls
uniform vec3 iBackgroundColor;
uniform vec3 iColor1;
uniform vec3 iColor2;
uniform vec3 iColor3;
uniform float iSpeed;
uniform float iBloom;
uniform float iSpread;
uniform float iDensity;

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

mat2 rot(float a) {
    float s = sin(a);
    float c = cos(a);
    return mat2(c, -s, s, c);
}

vec2 flowA(vec2 p, float t, float spread) {
    vec2 q = p;
    q += vec2(sin(p.y * 1.70 + t * 0.82), cos(p.x * 1.38 - t * 1.07)) * (0.45 + spread * 0.85);
    q = rot(0.22 * sin(t * 0.20) + 0.12) * q;
    return q;
}

vec2 flowB(vec2 p, float t, float spread) {
    vec2 q = p;
    q += vec2(cos(p.y * 1.28 - t * 0.96), sin(p.x * 1.82 + t * 0.88)) * (0.40 + spread * 0.80);
    q = rot(-0.30 * cos(t * 0.16) - 0.14) * q;
    return q;
}

void main() {
    vec2 uv = (gl_FragCoord.xy - 0.5 * iResolution.xy) / iResolution.y;
    float t = iTime * iSpeed * 0.60;
    float bloom = clamp(iBloom * 0.01, 0.0, 1.0);
    float spread = clamp(iSpread * 0.01, 0.0, 1.0);
    float density = clamp(iDensity * 0.01, 0.10, 1.0);

    float gridScale = mix(9.0, 30.0, density);
    vec2 domain = uv * gridScale;
    float warpScale = 0.82 + spread * 0.75;

    vec2 streamA = flowA(domain * warpScale, t, spread);
    vec2 streamB = flowB(domain * vec2(warpScale * 0.95, warpScale), t, spread);

    vec2 baseA = floor(streamA);
    vec2 baseB = floor(streamB);

    vec3 accum = vec3(0.0);
    float energyA = 0.0;
    float energyB = 0.0;
    float bloomGain = mix(0.22, 1.25, bloom);

    for (int oy = -1; oy <= 1; oy++) {
        for (int ox = -1; ox <= 1; ox++) {
            vec2 offset = vec2(float(ox), float(oy));

            vec2 cellA = baseA + offset;
            vec2 seedA = hash22(cellA + 37.1);
            float phaseA = hash21(cellA + 141.7) * 6.2831853;
            vec2 orbitA = vec2(
                sin(t * (0.95 + seedA.x * 0.90) + phaseA),
                cos(t * (1.10 + seedA.y * 0.85) - phaseA)
            );
            vec2 jitterA = (seedA - 0.5) * (0.85 + spread * 1.75);
            vec2 particleA = cellA + 0.5 + jitterA * 0.45 + orbitA * (0.22 + spread * 0.44);
            float distA = length(streamA - particleA);

            float coreA = smoothstep(0.22, 0.0, distA);
            coreA *= coreA;
            float haloA = exp(-distA * mix(16.0, 7.0, bloom));
            float twinkleA = 0.72 + 0.28 * sin(t * 8.5 + phaseA * 2.7 + seedA.x * 21.0);
            float contribA = coreA * (1.15 + twinkleA) + haloA * 0.20 * bloomGain;
            accum += iColor1 * contribA;
            energyA += coreA;

            vec2 cellB = baseB + offset;
            vec2 seedB = hash22(cellB + 91.3);
            float phaseB = hash21(cellB + 269.9) * 6.2831853;
            vec2 orbitB = vec2(
                cos(-t * (1.02 + seedB.y * 0.92) + phaseB),
                sin(-t * (0.88 + seedB.x * 0.86) - phaseB)
            );
            vec2 jitterB = (seedB - 0.5) * (0.92 + spread * 1.95);
            vec2 particleB = cellB + 0.5 + jitterB * 0.48 + orbitB * (0.25 + spread * 0.48);
            float distB = length(streamB - particleB);

            float coreB = smoothstep(0.21, 0.0, distB);
            coreB *= coreB;
            float haloB = exp(-distB * mix(17.0, 7.4, bloom));
            float twinkleB = 0.70 + 0.30 * sin(-t * 9.2 + phaseB * 2.1 + seedB.y * 23.0);
            float contribB = coreB * (1.20 + twinkleB) + haloB * 0.19 * bloomGain;
            accum += iColor2 * contribB;
            energyB += coreB;
        }
    }

    float overlap = sqrt(max(0.0, energyA * energyB));
    float pulse = 0.65 + 0.35 * sin(t * 2.3 + length(uv) * 9.5);
    accum += iColor3 * overlap * (1.45 + bloom * 2.75) * pulse;

    float edgeFade = 1.0 - smoothstep(0.18, 1.28 + spread * 0.20, length(uv));
    accum *= 0.75 + edgeFade * 0.45;

    vec3 color = iBackgroundColor + accum * (1.0 + density * 1.35);
    color = 1.0 - exp(-color * mix(1.05, 1.85, bloom));

    fragColor = vec4(clamp(color, 0.0, 1.0), 1.0);
}
