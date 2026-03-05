#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iSpeed;
uniform float iDensity;
uniform float iSize;
uniform float iDrift;
uniform float iRefraction;
uniform float iGlow;
uniform int iScene;
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

void palette(int id, out vec3 bg0, out vec3 bg1, out vec3 tint, out vec3 rim) {
    if (id == 0) {
        bg0 = vec3(0.02, 0.14, 0.22);
        bg1 = vec3(0.01, 0.03, 0.09);
        tint = vec3(0.38, 0.82, 1.0);
        rim = vec3(0.82, 0.96, 1.0);
        return;
    }
    if (id == 1) {
        bg0 = vec3(0.20, 0.03, 0.11);
        bg1 = vec3(0.04, 0.01, 0.08);
        tint = vec3(1.0, 0.35, 0.72);
        rim = vec3(1.0, 0.88, 0.62);
        return;
    }
    if (id == 2) {
        bg0 = vec3(0.05, 0.02, 0.16);
        bg1 = vec3(0.01, 0.0, 0.05);
        tint = vec3(0.62, 0.34, 1.0);
        rim = vec3(0.78, 1.0, 0.95);
        return;
    }
    if (id == 3) {
        bg0 = vec3(0.18, 0.12, 0.22);
        bg1 = vec3(0.06, 0.04, 0.12);
        tint = vec3(0.74, 0.76, 1.0);
        rim = vec3(1.0, 0.92, 0.96);
        return;
    }
    bg0 = vec3(0.05, 0.08, 0.20);
    bg1 = vec3(0.02, 0.02, 0.10);
    tint = vec3(0.66, 0.66, 1.0);
    rim = vec3(0.92, 0.90, 1.0);
}

void sceneParams(int scene, out float riseMul, out float driftMul, out float popMul) {
    if (scene == 0) {
        riseMul = 0.65;
        driftMul = 0.45;
        popMul = 0.35;
        return;
    }
    if (scene == 2) {
        riseMul = 1.45;
        driftMul = 1.25;
        popMul = 1.25;
        return;
    }
    riseMul = 1.0;
    driftMul = 0.95;
    popMul = 0.75;
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    vec2 ap = vec2(iResolution.x / iResolution.y, 1.0);
    vec2 p = (uv - 0.5) * ap;

    vec3 bg0;
    vec3 bg1;
    vec3 tint;
    vec3 rimCol;
    palette(iPalette, bg0, bg1, tint, rimCol);

    float riseMul;
    float driftMul;
    float popMul;
    sceneParams(iScene, riseMul, driftMul, popMul);

    float t = iTime * (0.35 + iSpeed * 0.2);
    vec3 col = mix(bg0, bg1, smoothstep(-0.55, 0.8, p.y));

    // Add soft underwater/soda turbulence in the background.
    float bgNoise = noise(uv * vec2(5.0, 3.0) + vec2(t * 0.07, -t * 0.05));
    col += tint * (bgNoise * 0.04);

    int bubbleCount = 8 + int(iDensity * 0.34);
    float baseSize = mix(0.02, 0.09, iSize * 0.01);
    float drift = iDrift * 0.01 * driftMul;
    float refract = iRefraction * 0.01;

    for (int i = 0; i < 48; i++) {
        if (i >= bubbleCount) break;
        float fi = float(i);

        float s1 = hash11(fi * 17.7);
        float s2 = hash11(fi * 31.9 + 2.0);
        float s3 = hash11(fi * 9.3 + 6.0);

        float depth = mix(0.35, 1.0, s3);
        float radius = baseSize * mix(0.55, 1.45, s2) * depth;

        float rise = fract(s1 + t * (0.045 + s2 * 0.09) * riseMul);
        float y = 1.15 - rise * 1.45;

        float xBase = s2 * 1.1 - 0.05;
        float sway = sin(t * (0.55 + s3) + fi * 3.4) * (0.02 + drift * 0.22);
        float swirl = sin(t * (0.2 + s1 * 0.5) + y * 14.0) * (0.01 + drift * 0.08);
        float x = xBase + sway + swirl;

        vec2 center = vec2(x, y);
        vec2 d = (uv - center) * ap;
        float dist = length(d);

        float bubble = smoothstep(radius, radius * 0.92, dist);
        float inner = smoothstep(radius * 0.78, radius * 0.35, dist);
        float rim = clamp(bubble - inner, 0.0, 1.0);

        // Refraction-like inner variation.
        vec2 n = d / max(dist, 0.001);
        float refr = noise((uv + n * refract * 0.03) * vec2(8.0, 6.0) + vec2(t * 0.1, -t * 0.07));
        float iridescence = 0.4 + 0.6 * sin(fi + refr * 6.28318 + t * 0.4);
        vec3 bubbleTint = mix(tint * 0.45, tint * (0.75 + iridescence * 0.35), inner * 0.7);

        // Specular highlight near upper-left.
        vec2 hPos = center + vec2(-radius * 0.25, -radius * 0.28);
        float spec = smoothstep(radius * 0.26, 0.0, length((uv - hPos) * ap));

        col += bubbleTint * inner * 0.55 * depth;
        col += rimCol * rim * (0.45 + iGlow * 0.006);
        col += vec3(1.0) * spec * 0.6 * depth;

        // Tiny pop flashes near top.
        if (y < 0.02 && hash11(fi + floor(t * 7.0)) > 0.92 - popMul * 0.12) {
            col += rimCol * (0.08 + iGlow * 0.002);
        }
    }

    // Atmosphere bloom.
    float bloom = smoothstep(0.4, -0.65, p.y) * (0.05 + iGlow * 0.002);
    col += tint * bloom * 0.4;

    col = 1.0 - exp(-col * 1.22);
    col = pow(clamp(col, 0.0, 1.0), vec3(0.94));

    fragColor = vec4(col, 1.0);
}
