#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

// Controls
uniform float iSpeed;
uniform float iDepth;
uniform float iCausticIntensity;
uniform float iCurrentStrength;
uniform int iPalette;

// ── Noise ──────────────────────────────────────────────────────────────

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

vec2 hash22(vec2 p) {
    p = fract(p * vec2(443.8975, 397.2973));
    p += dot(p, p.yx + 19.19);
    return fract(vec2(p.x * p.y, p.y * p.x));
}

float vnoise(vec2 x) {
    vec2 i = floor(x);
    vec2 f = fract(x);
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
    for (int i = 0; i < 6; i++) {
        if (i >= octaves) break;
        sum += amp * vnoise(p * freq);
        freq *= 2.0;
        amp *= 0.5;
    }
    return sum;
}

// ── Palettes ───────────────────────────────────────────────────────────

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    // Ocean (default)
    if (id == 0) return iqPalette(t, vec3(0.0, 0.15, 0.3), vec3(0.1, 0.3, 0.3), vec3(0.6, 0.8, 1.0), vec3(0.5, 0.6, 0.7));
    // Deep Sea
    if (id == 1) return iqPalette(t, vec3(0.0, 0.05, 0.15), vec3(0.0, 0.2, 0.3), vec3(0.5, 0.7, 0.8), vec3(0.6, 0.7, 0.8));
    // Aurora
    if (id == 2) return iqPalette(t, vec3(0.2, 0.5, 0.4), vec3(0.3, 0.4, 0.4), vec3(0.8, 0.7, 0.9), vec3(0.6, 0.3, 0.7));
    // SilkCircuit
    if (id == 3) return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
    // Midnight
    if (id == 4) return iqPalette(t, vec3(0.05, 0.0, 0.15), vec3(0.15, 0.1, 0.3), vec3(0.6, 0.5, 0.8), vec3(0.7, 0.6, 0.8));
    return iqPalette(t, vec3(0.0, 0.15, 0.3), vec3(0.1, 0.3, 0.3), vec3(0.6, 0.8, 1.0), vec3(0.5, 0.6, 0.7));
}

// ── Caustics ───────────────────────────────────────────────────────────

float caustics(vec2 p, float time) {
    // Worley-based caustic pattern
    vec2 i = floor(p);
    vec2 f = fract(p);

    float minDist = 1.0;
    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            vec2 neighbor = vec2(float(x), float(y));
            vec2 point = hash22(i + neighbor);
            point = 0.5 + 0.5 * sin(time * 0.6 + point * 6.28);
            float d = length(neighbor + point - f);
            minDist = min(minDist, d);
        }
    }

    // Sharp caustic lines at cell boundaries
    return pow(minDist, 0.5);
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    float time = iTime * iSpeed * 0.15;

    // Depth gradient — darker at bottom
    float depthFactor = iDepth * 0.01;
    float verticalDepth = mix(0.5, 1.0, depthFactor) - uv.y * depthFactor * 0.6;

    // ── Water current distortion ───────────────────────────────────────
    float currentAmt = iCurrentStrength * 0.004;
    vec2 currentOffset = vec2(
        fbm(vec2(uv.y * 3.0 + time, time * 0.3), 4) * currentAmt,
        fbm(vec2(uv.x * 2.0 + time * 0.5, time * 0.2 + 7.0), 3) * currentAmt * 0.5
    );
    vec2 distortedUV = uv + currentOffset;

    // ── Base water color ───────────────────────────────────────────────
    float colorT = distortedUV.x * 0.3 + distortedUV.y * 0.2 + time * 0.02;
    vec3 waterColor = paletteColor(colorT, iPalette);

    // Depth attenuation — deeper = darker, more blue-shifted
    float depthAtten = 0.2 + 0.8 * verticalDepth;
    waterColor *= depthAtten;

    // ── Layered sine waves ─────────────────────────────────────────────
    float waves = 0.0;
    for (int i = 0; i < 4; i++) {
        float fi = float(i);
        float waveFreq = 3.0 + fi * 2.5;
        float waveSpeed = time * (0.8 + fi * 0.3);
        float waveAngle = 0.3 * fi;
        vec2 waveDir = vec2(cos(waveAngle), sin(waveAngle));
        waves += sin(dot(distortedUV, waveDir) * waveFreq + waveSpeed) * (0.3 - fi * 0.06);
    }
    waves = waves * 0.5 + 0.5;

    // ── Caustics ───────────────────────────────────────────────────────
    float causticScale = 6.0 + iDepth * 0.04;
    float c1 = caustics(distortedUV * causticScale, time);
    float c2 = caustics(distortedUV * causticScale * 1.3 + vec2(3.7, 1.2), time * 1.2);

    // Overlay two caustic layers for complexity
    float causticPattern = min(c1, c2);
    causticPattern = smoothstep(0.0, 0.5, causticPattern);

    // Caustics are brighter near the surface
    float surfaceProximity = smoothstep(0.0, 0.8, uv.y);
    float causticBrightness = causticPattern * surfaceProximity * iCausticIntensity * 0.015;

    vec3 causticColor = paletteColor(colorT + 0.2, iPalette) * 1.5;

    // ── Floating particles (suspended sediment) ────────────────────────
    float particles = 0.0;
    for (int i = 0; i < 3; i++) {
        float fi = float(i);
        vec2 particleUV = distortedUV * (20.0 + fi * 15.0);
        particleUV.y += time * (0.3 + fi * 0.1);
        particleUV.x += sin(time * 0.5 + fi * 2.0) * 0.5;

        vec2 cell = floor(particleUV);
        vec2 local = fract(particleUV) - 0.5;
        float rng = hash21(cell + fi * 100.0);

        if (rng > 0.92) {
            vec2 offset = hash22(cell) - 0.5;
            float dist = length(local - offset * 0.3);
            float size = 0.02 + rng * 0.03;
            particles += smoothstep(size, size * 0.3, dist) * (0.2 + rng * 0.3);
        }
    }

    // ── Compose ────────────────────────────────────────────────────────
    vec3 col = waterColor * (0.7 + waves * 0.3);
    col += causticColor * causticBrightness;
    col += vec3(0.6, 0.8, 1.0) * particles * 0.15 * depthAtten;

    // Light rays from above (god rays)
    float rayX = sin(distortedUV.x * 8.0 + time * 0.3) * 0.5 + 0.5;
    float ray = pow(rayX, 8.0) * surfaceProximity * 0.15 * (1.0 - depthFactor * 0.5);
    col += paletteColor(0.5, iPalette) * ray;

    // Vignette
    float vignette = 1.0 - 0.3 * length((uv - 0.5) * vec2(1.5, 1.0));
    col *= vignette;

    // Tonemapping
    col = col / (1.0 + col * 0.5);

    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
