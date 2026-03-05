/**
 * Hypercolor GLSL Noise Functions
 * Hash, value noise, Worley, FBM, and domain warping.
 */

// ── Hash Functions ──────────────────────────────────────────────────────

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

float hash21_hq(vec2 p) {
    p = fract(p * vec2(233.34, 851.73));
    p += dot(p, p + 23.45);
    return fract(p.x * p.y);
}

vec2 hash22(vec2 p) {
    p = fract(p * vec2(443.8975, 397.2973));
    p += dot(p, p.yx + 19.19);
    return fract(vec2(p.x * p.y, p.y * p.x));
}

vec3 hash33(vec3 p) {
    p = fract(p * vec3(443.8975, 397.2973, 491.1871));
    p += dot(p, p.zxy + 19.19);
    return fract(vec3(p.x * p.y, p.y * p.z, p.z * p.x));
}

// ── Value Noise ─────────────────────────────────────────────────────────

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

float vnoise3(vec3 x) {
    vec3 i = floor(x);
    vec3 f = fract(x);
    f = f * f * (3.0 - 2.0 * f);

    float n = i.x + i.y * 57.0 + i.z * 113.0;

    float a = hash11(n);
    float b = hash11(n + 1.0);
    float c = hash11(n + 57.0);
    float d = hash11(n + 58.0);
    float e = hash11(n + 113.0);
    float f1 = hash11(n + 114.0);
    float g = hash11(n + 170.0);
    float h = hash11(n + 171.0);

    return mix(
        mix(mix(a, b, f.x), mix(c, d, f.x), f.y),
        mix(mix(e, f1, f.x), mix(g, h, f.x), f.y),
        f.z
    );
}

// ── Worley / Cellular Noise ─────────────────────────────────────────────

vec2 worley(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);

    float F1 = 1.0;
    float F2 = 1.0;

    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            vec2 neighbor = vec2(float(x), float(y));
            vec2 point = hash22(i + neighbor);
            vec2 diff = neighbor + point - f;
            float d = dot(diff, diff);

            if (d < F1) {
                F2 = F1;
                F1 = d;
            } else if (d < F2) {
                F2 = d;
            }
        }
    }

    return vec2(sqrt(F1), sqrt(F2));
}

// ── Fractional Brownian Motion ──────────────────────────────────────────

float fbm2(vec2 p, int octaves) {
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

float fbm3(vec3 p, int octaves) {
    float sum = 0.0;
    float amp = 0.5;
    float freq = 1.0;

    for (int i = 0; i < 8; i++) {
        if (i >= octaves) break;
        sum += amp * vnoise3(p * freq);
        freq *= 2.0;
        amp *= 0.5;
    }

    return sum;
}

float ridgedFbm(vec3 p, int octaves) {
    float sum = 0.0;
    float amp = 0.5;
    float freq = 1.0;

    for (int i = 0; i < 8; i++) {
        if (i >= octaves) break;
        float n = vnoise3(p * freq);
        n = 1.0 - abs(n * 2.0 - 1.0);
        sum += amp * n;
        freq *= 2.0;
        amp *= 0.5;
    }

    return sum;
}

// ── Domain Warping ──────────────────────────────────────────────────────

vec2 domainWarp(vec2 p, float strength, float scale) {
    float n1 = vnoise(p * scale);
    float n2 = vnoise(p * scale + vec2(5.2, 1.3));
    return p + vec2(n1, n2) * strength;
}

vec2 turbulentWarp(vec2 p, float strength, float scale, int octaves) {
    float n1 = fbm2(p * scale, octaves);
    float n2 = fbm2(p * scale + vec2(5.2, 1.3), octaves);
    return p + vec2(n1, n2) * strength;
}
