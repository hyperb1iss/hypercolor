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

float fbm3(vec2 p) {
    float sum = 0.0;
    float amp = 0.5;
    for (int i = 0; i < 3; i++) {
        sum += amp * vnoise(p);
        p = p * 2.02 + vec2(13.7, 7.1);
        amp *= 0.5;
    }
    return sum;
}

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

vec3 triGradient(float t, vec3 a, vec3 b, vec3 c) {
    t = saturate(t);
    if (t < 0.5) return mix(a, b, t * 2.0);
    return mix(b, c, (t - 0.5) * 2.0);
}

vec3 paletteColor(float t, int palette) {
    if (palette == 0) {
        return triGradient(t, vec3(0.02, 0.005, 0.002), vec3(0.82, 0.18, 0.03), vec3(0.96, 0.48, 0.08));
    }
    if (palette == 1) {
        return triGradient(t, vec3(0.015, 0.05, 0.028), vec3(0.26, 0.88, 0.18), vec3(0.62, 0.90, 0.30));
    }
    if (palette == 2) {
        return triGradient(t, vec3(0.03, 0.02, 0.08), vec3(0.84, 0.18, 0.92), vec3(0.36, 0.88, 0.90));
    }
    if (palette == 3) {
        return triGradient(t, vec3(0.055, 0.025, 0.045), vec3(0.82, 0.20, 0.60), vec3(0.94, 0.56, 0.22));
    }
    return triGradient(t, vec3(0.03, 0.045, 0.028), vec3(0.54, 0.80, 0.16), vec3(0.90, 0.48, 0.18));
}

vec2 flowVector(vec2 p, float time, float spread, int scene) {
    vec2 dir = sceneDirection(scene);
    vec2 side = vec2(-dir.y, dir.x);

    float bend = sin(dot(p, side * 3.6) + time * (1.3 + spread * 1.8));
    float curl = vnoise(p * 1.9 + vec2(time * 0.18, -time * 0.12)) - 0.5;
    vec2 flow = dir + side * (bend * (0.10 + spread * 0.18) + curl * (0.22 + spread * 0.34));
    if (scene == 2) {
        vec2 center = p - vec2(0.35, 0.28);
        flow += vec2(-center.y, center.x) * (0.14 + spread * 0.40) / (0.26 + dot(center, center) * 3.2);
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
    adv -= baseFlow * time * 0.070 * lift;
    adv += side * (vnoise(p * vec2(2.8, 2.2) + vec2(time * 0.24, -time * 0.18)) - 0.5) * (0.05 + spread * 0.09);

    float floorHeat = 1.0 - smoothstep(-0.06, 1.04, uv.y);
    float emberBed = fbm3(adv * vec2(2.5, 1.8) + vec2(0.0, -time * (0.42 + lift * 0.18)));
    float ridges = 1.0 - abs(vnoise(adv * vec2(5.0, 3.2) + vec2(time * 0.28, -time * 0.70)) * 2.0 - 1.0);
    float tongues = smoothstep(0.48 - intensity * 0.16, 0.96, emberBed * 0.82 + ridges * 0.38 + floorHeat * (0.22 + intensity * 0.18));

    float haze = fbm3(adv * vec2(1.4, 1.1) + vec2(-time * 0.08, time * 0.05));
    haze *= (0.02 + glow * 0.06) * (0.16 + floorHeat * 0.46);

    float heat = saturate(tongues * (0.40 + intensity * 0.68) + haze * (0.12 + spread * 0.10));
    vec3 col = paletteColor(heat, iPalette) * pow(max(heat, 0.0), 1.22) * (0.26 + intensity * 0.66);

    float streamAxis = dot(adv, baseFlow * (6.2 + spread * 4.0) + side * (1.2 + swirl * 1.2));
    float streamNoise = vnoise(adv * vec2(5.4, 2.4) + vec2(time * 0.84, -time * 0.32));
    float streaks = smoothstep(0.68, 0.94, fract(streamAxis - time * (1.45 + spread * 1.9) + streamNoise * 0.9));
    streaks *= smoothstep(0.05, 0.88, floorHeat + 0.24);
    col += paletteColor(0.34 + streamNoise * 0.58, iPalette) * streaks * (0.04 + intensity * 0.11);

    vec3 fleckColor = vec3(0.0);
    float fleckGlow = 0.0;
    for (int layer = 0; layer < 2; layer++) {
        float lf = float(layer);
        float layerMix = lf;
        float scale = mix(22.0, 52.0, layerMix);
        float layerSpeed = (0.75 + lf * 0.34) * lift;

        vec2 layerFlow = normalize(baseFlow + side * sin(time * (0.7 + lf * 0.4) + p.y * (2.2 + lf * 1.1)) * (0.10 + spread * (0.16 + layerMix * 0.10)));
        vec2 layerSide = vec2(-layerFlow.y, layerFlow.x);

        vec2 q = p * scale;
        q -= layerFlow * time * (5.2 + lf * 2.6) * layerSpeed;
        q += layerSide * sin((p.y + lf * 0.17) * TAU * 1.20 + time * (1.45 + lf * 0.36)) * (0.20 + spread * (0.24 + layerMix * 0.18));

        vec2 qf = q - 0.5;
        vec2 cell = floor(qf);
        vec2 local = fract(qf);
        float spawnRate = (0.03 + density * 0.08) * (1.0 - layerMix * 0.16);

        for (int oy = 0; oy <= 1; oy++) {
            for (int ox = 0; ox <= 1; ox++) {
                vec2 cid = cell + vec2(float(ox), float(oy));
                float seed = hash21(cid + vec2(37.1 + lf * 21.3, 9.3 + lf * 17.7));
                if (seed > spawnRate) continue;

                vec2 jitter = hash22(cid + vec2(19.2 + lf * 4.3, 71.8 + lf * 5.9));
                vec2 rel = local - vec2(float(ox), float(oy)) - jitter;

                float along = dot(rel, layerFlow);
                float across = dot(rel, layerSide);

                float tailAlong = max(along + 0.10 + lf * 0.03, 0.0);
                float core = 1.0 - (across * across * (18.0 + lf * 5.0) + along * along * (10.0 + lf * 2.0));
                core = max(core, 0.0);
                core *= core;
                float tail = 1.0 - (abs(across) * (4.2 + lf * 1.2) + tailAlong * (2.4 + lf * 0.8));
                tail = max(tail, 0.0);
                float fleck = core + tail * (0.28 + spread * 0.36);

                float flicker = 0.68 + 0.32 * sin(time * (7.4 + lf * 1.8) + seed * 67.0);
                float altitude = clamp(1.08 - uv.y * (0.60 + layerMix * 0.42), 0.1, 1.0);
                float hotness = saturate(0.18 + altitude * 0.78 + seed * 0.32);

                float alpha = fleck * flicker * altitude * (0.22 - layerMix * 0.06) * (0.62 + intensity * 0.75);
                fleckColor += paletteColor(hotness, iPalette) * alpha;
                fleckGlow += alpha;
            }
        }
    }

    col += fleckColor;

    float veil = vnoise(p * vec2(1.2, 1.6) + vec2(time * 0.09, -time * 0.13));
    veil *= smoothstep(0.0, 0.95, uv.y) * (0.015 + spread * 0.04 + glow * 0.04);
    col += paletteColor(0.30 + veil * 0.35, iPalette) * veil;

    float bloom = heat * (0.18 + glow * 0.48) + fleckGlow * (0.10 + glow * 0.24);
    col += paletteColor(0.56 + heat * 0.3, iPalette) * bloom * 0.15;

    vec2 center = uv - vec2(0.5, 0.44);
    float vignette = 1.0 - dot(center * vec2(1.1, 0.95), center * vec2(1.1, 0.95)) * 1.35;
    col *= clamp(vignette, 0.2, 1.0);

    col = col / (vec3(1.0) + col * 0.8);
    col = pow(col, vec3(0.95));

    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
