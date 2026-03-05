#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iSpeed;
uniform float iIntensity;
uniform float iWarpStrength;
uniform float iStarBrightness;
uniform float iCurtainHeight;
uniform int iPalette;

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

float vnoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    f = f * f * (3.0 - 2.0 * f);

    float a = hash21(i);
    float b = hash21(i + vec2(1.0, 0.0));
    float c = hash21(i + vec2(0.0, 1.0));
    float d = hash21(i + vec2(1.0, 1.0));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

float fbm(vec2 p, int octaves) {
    float sum = 0.0;
    float amp = 0.5;
    float freq = 1.0;
    for (int i = 0; i < 8; i++) {
        if (i >= octaves) break;
        sum += amp * vnoise(p * freq);
        freq *= 2.0;
        amp *= 0.5;
    }
    return sum;
}

vec2 domainWarp(vec2 p, float strength, float scale) {
    float nx = fbm(p * scale + vec2(1.7, 9.2), 4);
    float ny = fbm(p * scale + vec2(8.3, 2.8), 4);
    return p + vec2(nx - 0.5, ny - 0.5) * strength;
}

mat2 rot(float a) {
    float c = cos(a);
    float s = sin(a);
    return mat2(c, -s, s, c);
}

float tri(float x) {
    return clamp(abs(fract(x) - 0.5), 0.01, 0.49);
}

vec2 tri2(vec2 p) {
    return vec2(tri(p.x) + tri(p.y), tri(p.y + tri(p.x)));
}

float triNoise2d(vec2 p, float speed, float time) {
    float z = 1.7;
    float z2 = 2.5;
    float rz = 0.0;

    p *= rot(p.x * 0.06);
    vec2 bp = p;
    const mat2 m2 = mat2(0.95534, -0.29552, 0.29552, 0.95534);

    for (int i = 0; i < 5; i++) {
        vec2 dg = tri2(bp * 1.85) * 0.75;
        dg *= rot(time * speed);

        p -= dg / z2;
        bp *= 1.3;
        z2 *= 0.52;
        z *= 0.47;
        p *= 1.22;
        p = m2 * p;

        rz += tri(p.x + tri(p.y)) * z;
    }

    return clamp(1.0 / pow(rz * 30.0, 1.25), 0.0, 1.0);
}

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    if (id == 0) {
        // Physically inspired northern lights: green dominant with magenta/purple accents.
        float primary = 0.5 + 0.5 * sin(6.28318 * (t * 0.55));
        float secondary = 0.5 + 0.5 * sin(6.28318 * (t * 0.87 + 0.22));
        vec3 emerald = vec3(0.08, 0.96, 0.47);
        vec3 magenta = vec3(0.93, 0.32, 0.84);
        vec3 violet = vec3(0.60, 0.27, 0.97);
        vec3 base = mix(emerald, magenta, 0.18 + primary * 0.34);
        return mix(base, violet, 0.14 + secondary * 0.30);
    }
    if (id == 1) return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
    if (id == 2) return iqPalette(t, vec3(0.5, 0.2, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 1.0), vec3(0.8, 0.1, 0.6));
    if (id == 3) return iqPalette(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 1.0), vec3(0.0, 0.1, 0.2));
    if (id == 4) return iqPalette(t, vec3(0.5, 0.6, 0.8), vec3(0.2, 0.3, 0.2), vec3(0.6, 0.8, 1.0), vec3(0.0, 0.1, 0.3));
    if (id == 5) return iqPalette(t, vec3(0.5, 0.2, 0.0), vec3(0.5, 0.4, 0.2), vec3(1.0, 0.7, 0.4), vec3(0.0, 0.15, 0.2));
    if (id == 6) return iqPalette(t, vec3(0.6, 0.4, 0.7), vec3(0.3, 0.3, 0.3), vec3(0.6, 0.8, 1.0), vec3(0.7, 0.3, 0.6));
    if (id == 7) return iqPalette(t, vec3(0.0, 0.3, 0.0), vec3(0.0, 0.5, 0.0), vec3(1.0, 1.0, 1.0), vec3(0.0, 0.0, 0.0));
    return iqPalette(t, vec3(0.2, 0.5, 0.4), vec3(0.3, 0.4, 0.4), vec3(0.8, 0.7, 0.9), vec3(0.6, 0.3, 0.7));
}

float starField(vec2 uv, float time) {
    vec2 denseUV = uv * vec2(180.0, 130.0);
    vec2 cell = floor(denseUV);
    float rng = hash21(cell);
    if (rng > 0.035) return 0.0;

    vec2 local = fract(denseUV) - 0.5;
    vec2 jitter = vec2(hash21(cell + 1.3), hash21(cell + 7.9)) - 0.5;
    float dist = length(local - jitter * 0.45);

    float twinkle = 0.55 + 0.45 * sin(time * (1.2 + rng * 3.5) + rng * 15.0);
    float size = mix(0.02, 0.045, rng * 18.0);
    return (1.0 - smoothstep(0.0, size, dist)) * twinkle;
}

vec3 auroraLayer(vec2 p, float time, float layer, float baseHeight) {
    float depth = layer / 5.0;
    float warp = (0.06 + iWarpStrength * 0.0038) * (1.0 + depth * 0.65);

    vec2 q = p;
    q.x *= 1.2 + depth * 0.6;
    q.y *= 0.75 + depth * 0.16;
    q.x += time * (0.05 + depth * 0.05);
    q = domainWarp(q + vec2(depth * 2.7, 0.0), warp, 1.0 + depth * 0.8);

    float ridgeNoise = fbm(vec2(q.x * 1.35 + depth * 4.1, time * 0.04 + depth * 2.3), 5) - 0.5;
    float ridge = baseHeight + ridgeNoise * (0.38 - depth * 0.08);
    float drop = ridge - p.y;

    float curtain = smoothstep(-0.05, 0.12, drop) * (1.0 - smoothstep(0.84, 1.08, drop));
    float rays = triNoise2d(vec2(q.x * 2.3 + depth * 6.0, p.y * 0.95 - depth * 1.9), 0.08 + depth * 0.04, time);
    float filament = pow(rays, 1.35);
    float shimmer = 0.68 + 0.32 * sin(time * 1.3 + q.x * 4.5 + depth * 5.7);
    float glow = exp(-abs(drop - 0.16) * 7.0);

    float intensity = curtain * filament * shimmer * (1.0 - depth * 0.12);
    intensity += glow * filament * 0.15;

    float colorT = depth * 0.18 + q.x * 0.08 + filament * 0.14 + time * 0.02;
    vec3 col = paletteColor(colorT, iPalette);
    if (iPalette == 0) {
        // Vertical color structure: green curtain body, magenta mid ribbons, purple crown.
        float crown = smoothstep(-0.02, 0.16, drop) * (1.0 - smoothstep(0.16, 0.34, drop));
        float ribbon = smoothstep(0.05, 0.36, drop) * (1.0 - smoothstep(0.36, 0.70, drop));
        float body = smoothstep(0.18, 0.80, drop) * (1.0 - smoothstep(0.80, 1.22, drop));
        float sweep = 0.5 + 0.5 * sin(q.x * 5.0 + time * 0.8 + layer * 1.1);
        float striation = 0.5 + 0.5 * sin(q.x * 10.5 + time * 1.05 + layer * 2.4 + filament * 4.2);

        vec3 greenCore = vec3(0.07, 0.97, 0.45);
        vec3 magentaRibbon = vec3(0.94, 0.32, 0.84);
        vec3 purpleCrown = vec3(0.62, 0.30, 0.98);

        vec3 physical = greenCore * (0.50 + 0.35 * body);
        physical += magentaRibbon * (0.12 + 0.48 * ribbon * sweep);
        physical += purpleCrown * (0.08 + 0.35 * crown * striation);

        col = mix(col, physical, 0.90);
        col = mix(col, magentaRibbon, 0.24 * ribbon * striation);
        col = mix(col, purpleCrown, 0.18 * crown * (0.7 + 0.3 * sweep));
    }
    vec3 highlight = (iPalette == 0) ? vec3(0.62, 0.98, 0.84) : vec3(0.88, 0.98, 0.95);
    col = mix(col, highlight, pow(filament, 3.0) * 0.16);

    return col * intensity * (0.05 + iIntensity * 0.0115);
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    vec2 p = (uv - 0.5) * vec2(iResolution.x / iResolution.y, 1.0);
    float time = iTime * iSpeed * 0.32;

    vec3 skyZenith = vec3(0.005, 0.012, 0.03);
    vec3 skyBase = (iPalette == 0) ? vec3(0.024, 0.046, 0.08) : vec3(0.02, 0.05, 0.12);
    vec3 skyHorizon = mix(skyBase, paletteColor(0.18 + time * 0.02, iPalette) * 0.24, 0.6);
    float skyMix = smoothstep(-0.45, 0.7, p.y);
    vec3 col = mix(skyHorizon, skyZenith, skyMix);
    float lowMist = 1.0 - smoothstep(-0.42, -0.04, p.y);
    col += skyBase * lowMist * 0.12;

    float stars = starField(uv, iTime) * (iStarBrightness * 0.012);
    col += vec3(0.8, 0.9, 1.0) * stars;

    float baseHeight = mix(-0.08, 0.3, clamp(iCurtainHeight * 0.01, 0.0, 1.0));
    vec3 aur = vec3(0.0);
    for (int i = 0; i < 6; i++) {
        aur += auroraLayer(p, time, float(i), baseHeight);
    }

    // Horizon haze + volumetric bloom where curtains meet atmosphere.
    float horizonHaze = exp(-abs(p.y + 0.2) * 8.5);
    aur += paletteColor(0.52 + time * 0.01, iPalette) * horizonHaze * 0.06 * (iIntensity * 0.008);

    col += aur;
    if (iPalette == 0) {
        float upperTint = smoothstep(-0.10, 0.85, p.y);
        col += vec3(0.20, 0.08, 0.28) * upperTint * 0.08;
    }

    // Subtle reflection to avoid a dead lower half.
    vec2 rp = vec2(p.x, -p.y - 0.16);
    vec3 reflection = vec3(0.0);
    for (int i = 0; i < 3; i++) {
        reflection += auroraLayer(rp, time * 0.86, float(i), baseHeight - 0.12);
    }
    float waterMask = 1.0 - smoothstep(-0.48, -0.18, p.y);
    col += reflection * waterMask * 0.34;
    float seamGlow = exp(-abs(p.y + 0.18) * 14.0);
    col += paletteColor(0.34 + time * 0.015, iPalette) * seamGlow * 0.02;

    // Filmic tone mapping tuned to preserve colored highlights.
    col = max(col, vec3(0.0));
    col = 1.0 - exp(-col * (0.96 + iIntensity * 0.0032));
    col = pow(clamp(col, 0.0, 1.0), vec3(0.94));

    fragColor = vec4(col, 1.0);
}
