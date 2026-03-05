#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iSpeed;
uniform float iBlobCount;
uniform float iBlobSize;
uniform float iViscosity;
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

float noise(vec2 x) {
    vec2 i = floor(x);
    vec2 f = fract(x);
    f = f * f * (3.0 - 2.0 * f);
    float a = hash21(i);
    float b = hash21(i + vec2(1.0, 0.0));
    float c = hash21(i + vec2(0.0, 1.0));
    float d = hash21(i + vec2(1.0, 1.0));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

float fbm(vec2 p) {
    float sum = 0.0;
    float amp = 0.5;
    for (int i = 0; i < 5; i++) {
        sum += amp * noise(p);
        p *= 2.0;
        amp *= 0.5;
    }
    return sum;
}

void lavaPalette(int id, out vec3 bg, out vec3 a, out vec3 b, out vec3 rim) {
    if (id == 0) {
        bg = vec3(0.09, 0.03, 0.01);
        a = vec3(0.99, 0.35, 0.08);
        b = vec3(1.00, 0.76, 0.22);
        rim = vec3(1.00, 0.88, 0.52);
        return;
    }
    if (id == 1) {
        bg = vec3(0.02, 0.01, 0.08);
        a = vec3(0.95, 0.12, 0.83);
        b = vec3(0.12, 0.92, 1.00);
        rim = vec3(0.86, 0.98, 1.00);
        return;
    }
    if (id == 2) {
        bg = vec3(0.10, 0.03, 0.08);
        a = vec3(1.00, 0.29, 0.61);
        b = vec3(1.00, 0.64, 0.26);
        rim = vec3(1.00, 0.90, 0.62);
        return;
    }
    if (id == 3) {
        bg = vec3(0.02, 0.07, 0.03);
        a = vec3(0.22, 0.92, 0.44);
        b = vec3(0.68, 1.00, 0.26);
        rim = vec3(0.88, 1.00, 0.70);
        return;
    }
    bg = vec3(0.02, 0.05, 0.08);
    a = vec3(0.14, 0.58, 1.00);
    b = vec3(0.40, 0.95, 0.96);
    rim = vec3(0.80, 0.96, 1.00);
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    vec2 p = (uv - 0.5) * vec2(iResolution.x / iResolution.y, 1.0);
    float t = iTime * (0.45 + iSpeed * 0.15);

    vec3 bg;
    vec3 c1;
    vec3 c2;
    vec3 rimCol;
    lavaPalette(iPalette, bg, c1, c2, rimCol);

    // Lamp glass background with subtle vertical gradient and soft vignette.
    float v = smoothstep(-0.7, 0.8, p.y);
    vec3 col = mix(bg * 0.6, bg * 1.25, v);
    float vignette = smoothstep(1.35, 0.4, length(p * vec2(0.9, 1.2)));
    col *= vignette;

    float field = 0.0;
    float fieldHi = 0.0;
    float blobCount = clamp(iBlobCount, 1.0, 16.0);
    float baseR = mix(0.05, 0.18, iBlobSize * 0.01);

    for (int i = 0; i < 20; i++) {
        float fi = float(i);
        if (fi >= blobCount) break;

        float seed = hash11(fi * 11.73);
        float seed2 = hash11(fi * 23.91 + 7.0);
        float sizeMod = 0.65 + 0.85 * seed2;
        float r = baseR * sizeMod;

        float swirl = sin(t * (0.5 + seed * 0.6) + seed * 10.0);
        float x = (seed - 0.5) * 0.9 + sin(t * (0.22 + seed * 0.3) + seed * 13.0) * 0.16 + swirl * 0.03;

        float rise = fract(seed + t * (0.05 + seed * 0.08));
        float y = -0.68 + rise * 1.45 + sin(t * (0.7 + seed * 0.9) + seed2 * 11.0) * 0.05;

        vec2 d = p - vec2(x, y);
        d.y *= 1.15;
        float d2 = dot(d, d) + 0.002;

        float influence = (r * r) / d2;
        field += influence;
        fieldHi += influence * influence;
    }

    float viscosity = iViscosity * 0.01;
    float threshold = mix(2.1, 4.4, viscosity);
    float blobMask = smoothstep(threshold * 0.84, threshold * 1.06, field);
    float blobCore = smoothstep(threshold * 1.02, threshold * 1.8, field);
    float rim = clamp(smoothstep(threshold * 0.74, threshold * 1.0, field) - blobMask, 0.0, 1.0);

    float swirlTex = fbm(p * vec2(6.5, 4.8) + vec2(t * 0.2, -t * 0.35));
    float heightGrad = smoothstep(-0.65, 0.7, p.y);
    vec3 lavaColor = mix(c1, c2, clamp(0.12 + 0.68 * heightGrad + swirlTex * 0.28, 0.0, 1.0));
    lavaColor = mix(lavaColor, rimCol, rim * 0.9);

    col = mix(col, lavaColor, blobMask);
    col += lavaColor * blobCore * 0.35;

    float glowAmt = (0.18 + iGlow * 0.012);
    float bloom = smoothstep(threshold * 0.52, threshold * 0.95, field) * glowAmt;
    col += mix(c1, rimCol, 0.6) * bloom * 0.35;

    // Glass reflection sweep.
    float glass = exp(-abs(p.x + 0.28) * 12.0) * smoothstep(-0.45, 0.8, p.y) * 0.1;
    col += vec3(1.0, 0.95, 0.86) * glass;

    col = 1.0 - exp(-col * 1.2);
    col = pow(clamp(col, 0.0, 1.0), vec3(0.92));

    fragColor = vec4(col, 1.0);
}
