#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

// Controls
uniform float iSpeed;
uniform float iIntensity;
uniform float iEmberDensity;
uniform float iFlowSpread;
uniform float iGlow;
uniform int iPalette;
uniform int iScene;

const float TAU = 6.28318530718;

float saturate(float v) { return clamp(v, 0.0, 1.0); }

// ── Noise ──────────────────────────────────────────────────────────────

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 31.32);
    return fract((p3.x + p3.y) * p3.z);
}

vec2 hash22(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * vec3(0.1031, 0.0973, 0.1099));
    p3 += dot(p3, p3.yzx + 29.57);
    return fract((p3.xx + p3.yz) * p3.zy);
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
        freq *= 2.02;
        amp *= 0.5;
    }
    return sum;
}

// ── Scene vectors ──────────────────────────────────────────────────────

vec2 sceneDirection(int scene) {
    if (scene == 1) return normalize(vec2(0.95, 0.82));
    if (scene == 2) return normalize(vec2(0.30, 1.00));
    return normalize(vec2(0.14, 1.00));
}

float sceneLift(int scene) {
    if (scene == 1) return 0.82;
    if (scene == 2) return 1.12;
    return 1.00;
}

float sceneSwirl(int scene) {
    if (scene == 1) return 0.45;
    if (scene == 2) return 1.00;
    return 0.25;
}

// ── Palette ────────────────────────────────────────────────────────────

vec3 triGradient(float t, vec3 a, vec3 b, vec3 c) {
    t = saturate(t);
    if (t < 0.5) return mix(a, b, t * 2.0);
    return mix(b, c, (t - 0.5) * 2.0);
}

vec3 paletteColor(float t, int palette) {
    if (palette == 0) {
        return triGradient(t, vec3(0.02, 0.005, 0.002), vec3(0.88, 0.20, 0.03), vec3(1.00, 0.54, 0.08));
    }
    if (palette == 1) {
        return triGradient(t, vec3(0.015, 0.05, 0.028), vec3(0.30, 0.93, 0.20), vec3(0.76, 1.00, 0.82));
    }
    if (palette == 2) {
        return triGradient(t, vec3(0.03, 0.02, 0.08), vec3(0.88, 0.18, 0.96), vec3(0.50, 0.98, 0.93));
    }
    if (palette == 3) {
        return triGradient(t, vec3(0.055, 0.025, 0.045), vec3(0.88, 0.21, 0.64), vec3(1.00, 0.60, 0.22));
    }
    return triGradient(t, vec3(0.03, 0.045, 0.028), vec3(0.57, 0.84, 0.16), vec3(1.00, 0.63, 0.24));
}

// ── Flow field ─────────────────────────────────────────────────────────

vec2 flowVector(vec2 p, float time, float spread, int scene) {
    vec2 dir = sceneDirection(scene);
    vec2 side = vec2(-dir.y, dir.x);

    float nA = vnoise(p * 2.2 + vec2(time * 0.29, -time * 0.21));
    float nB = vnoise(p * 3.1 + vec2(-time * 0.24, time * 0.27));
    vec2 curl = vec2(nA - 0.5, nB - 0.5);

    float wave = sin(dot(p, side * 4.6) + time * (1.5 + spread * 2.4));
    vec2 bend = side * wave * (0.08 + spread * 0.26);

    vec2 flow = dir + curl * (0.18 + spread * 0.52) + bend;
    if (scene == 2) {
        vec2 center = p - vec2(0.35, 0.28);
        float radius = max(length(center), 0.08);
        flow += vec2(-center.y, center.x) * (0.18 + spread * 0.62) / (0.22 + radius * 1.8);
    }

    return normalize(flow + vec2(1e-4, 0.0));
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    vec2 p = uv;
    p.x *= iResolution.x / max(iResolution.y, 1.0);

    float time = iTime * max(iSpeed, 0.05) * 0.52;
    float intensity = saturate(iIntensity / 100.0);
    float density = saturate(iEmberDensity / 100.0);
    float spread = saturate(iFlowSpread / 100.0);
    float glow = saturate(iGlow / 100.0);

    vec2 baseFlow = flowVector(p * vec2(0.95, 1.45), time, spread, iScene);
    vec2 side = vec2(-baseFlow.y, baseFlow.x);
    float lift = sceneLift(iScene);
    float swirl = sceneSwirl(iScene);

    vec2 adv = p;
    adv -= baseFlow * time * 0.055 * lift;
    adv += side * (vnoise(p * vec2(3.6, 2.9) + vec2(time * 0.42, -time * 0.28)) - 0.5) * (0.03 + spread * 0.08);

    // Ember bed: ridged tongues rather than fog fill.
    float floorHeat = smoothstep(1.05, -0.08, uv.y);
    float emberBed = fbm(adv * vec2(3.4, 2.2) + vec2(0.0, -time * (0.55 + lift * 0.35)), 5);
    float ridges = 1.0 - abs(fbm(adv * vec2(5.4, 3.7) + vec2(time * 0.35, -time * 0.88), 4) * 2.0 - 1.0);
    float tongues = smoothstep(0.64 - intensity * 0.16, 1.04, emberBed * 0.88 + ridges * 0.42 + floorHeat * (0.28 + intensity * 0.14));

    float haze = fbm(adv * vec2(1.8, 1.35) + vec2(-time * 0.12, time * 0.07), 3);
    haze *= (0.03 + glow * 0.08) * (0.16 + floorHeat * 0.56);

    float heat = saturate(tongues * (0.38 + intensity * 0.72) + haze * (0.14 + spread * 0.12));
    vec3 col = paletteColor(heat, iPalette) * pow(max(heat, 0.0), 1.22) * (0.26 + intensity * 0.66);

    // Directional streamers to reinforce flow identity.
    float streamAxis = dot(adv, baseFlow * (7.5 + spread * 5.0) + side * (1.4 + swirl * 1.6));
    float streamNoise = fbm(adv * vec2(7.8, 2.9) + vec2(time * 1.18, -time * 0.46), 3);
    float streaks = smoothstep(0.68, 0.94, fract(streamAxis - time * (1.45 + spread * 1.9) + streamNoise * 0.9));
    streaks *= smoothstep(0.05, 0.88, floorHeat + 0.24);
    col += paletteColor(0.34 + streamNoise * 0.58, iPalette) * streaks * (0.04 + intensity * 0.11);

    // Crisp flecks with anisotropic tails aligned to local flow.
    vec3 fleckColor = vec3(0.0);
    float fleckGlow = 0.0;
    for (int layer = 0; layer < 4; layer++) {
        float lf = float(layer);
        float layerMix = lf / 3.0;
        float scale = mix(24.0, 74.0, layerMix);
        float layerSpeed = (0.75 + lf * 0.34) * lift;

        vec2 layerFlow = flowVector(p * (1.0 + lf * 0.18) + vec2(lf * 2.7, -lf * 1.9), time * (1.0 + lf * 0.2), spread, iScene);
        vec2 layerSide = vec2(-layerFlow.y, layerFlow.x);

        vec2 q = p * scale;
        q -= layerFlow * time * (5.6 + lf * 3.1) * layerSpeed;
        q += layerSide * sin((p.y + lf * 0.17) * TAU * 1.25 + time * (1.7 + lf * 0.45)) * (0.30 + spread * (0.42 + layerMix * 0.28));

        vec2 cell = floor(q);
        vec2 local = fract(q) - 0.5;
        float spawnRate = (0.02 + density * 0.10) * (1.0 - layerMix * 0.24);

        for (int oy = -1; oy <= 1; oy++) {
            for (int ox = -1; ox <= 1; ox++) {
                vec2 cid = cell + vec2(float(ox), float(oy));
                float seed = hash21(cid + vec2(37.1 + lf * 21.3, 9.3 + lf * 17.7));
                if (seed > spawnRate) continue;

                vec2 jitter = hash22(cid + vec2(19.2 + lf * 4.3, 71.8 + lf * 5.9)) - 0.5;
                vec2 rel = local - vec2(float(ox), float(oy)) - jitter * (0.82 + spread * 0.74);

                float along = dot(rel, layerFlow);
                float across = dot(rel, layerSide);

                float core = exp(-(across * across * (102.0 + lf * 26.0) + along * along * (56.0 + lf * 14.0)));
                float tail = exp(-(across * across * (64.0 + lf * 19.0) + pow(max(along + 0.10 + lf * 0.025, 0.0), 2.0) * (22.0 + lf * 9.0)));
                float fleck = smoothstep(0.18, 1.0, core + tail * (0.44 + spread * 0.52));

                float flicker = 0.64 + 0.36 * sin(time * (8.8 + lf * 2.1) + seed * 67.0);
                float altitude = clamp(1.08 - uv.y * (0.60 + layerMix * 0.42), 0.1, 1.0);
                float hotness = saturate(0.18 + altitude * 0.78 + seed * 0.32);

                float alpha = fleck * flicker * altitude * (0.36 - layerMix * 0.12) * (0.52 + intensity * 0.9);
                fleckColor += paletteColor(hotness, iPalette) * alpha;
                fleckGlow += alpha;
            }
        }
    }

    col += fleckColor;

    float veil = fbm(p * vec2(1.2, 1.6) + vec2(time * 0.09, -time * 0.13), 4);
    veil *= smoothstep(0.0, 0.95, uv.y) * (0.015 + spread * 0.04 + glow * 0.04);
    col += paletteColor(0.30 + veil * 0.35, iPalette) * veil;

    float bloom = heat * (0.26 + glow * 0.78) + fleckGlow * (0.16 + glow * 0.48);
    col += paletteColor(0.56 + heat * 0.3, iPalette) * bloom * 0.22;

    vec2 center = uv - vec2(0.5, 0.44);
    float vignette = 1.0 - dot(center * vec2(1.1, 0.95), center * vec2(1.1, 0.95)) * 1.35;
    col *= clamp(vignette, 0.2, 1.0);

    col = col / (vec3(1.0) + col * 0.8);
    col = pow(col, vec3(0.95));

    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
