#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iAudioBass;
uniform float iAudioMid;
uniform float iAudioTreble;
uniform float iAudioLevel;
uniform float iAudioBeatPulse;
uniform float iAudioSpectralFlux;

uniform float iSpeed;
uniform float iFlameHeight;
uniform float iTurbulence;
uniform float iIntensity;
uniform float iEmberAmount;
uniform int iPalette;
uniform int iScene;

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

float saturate(float v) {
    return clamp(v, 0.0, 1.0);
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

float ridge(float n) {
    return 1.0 - abs(n * 2.0 - 1.0);
}

vec3 fireRamp(float t, vec3 c0, vec3 c1, vec3 c2, vec3 c3) {
    t = saturate(t);
    vec3 col = mix(c0, c1, smoothstep(0.0, 0.35, t));
    col = mix(col, c2, smoothstep(0.24, 0.74, t));
    col = mix(col, c3, smoothstep(0.62, 1.0, t));
    return col;
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return fireRamp(t, vec3(0.02, 0.0, 0.0), vec3(0.70, 0.07, 0.0), vec3(1.0, 0.44, 0.05), vec3(0.86, 0.48, 0.14));
    if (id == 1) return fireRamp(t, vec3(0.01, 0.0, 0.0), vec3(0.78, 0.16, 0.02), vec3(1.0, 0.62, 0.06), vec3(0.88, 0.54, 0.17));
    if (id == 2) return fireRamp(t, vec3(0.03, 0.0, 0.05), vec3(0.58, 0.07, 0.72), vec3(0.17, 0.74, 1.0), vec3(0.58, 0.86, 0.98));
    if (id == 3) return fireRamp(t, vec3(0.01, 0.01, 0.0), vec3(0.28, 0.38, 0.0), vec3(0.86, 0.95, 0.12), vec3(0.82, 0.70, 0.22));
    if (id == 4) return fireRamp(t, vec3(0.01, 0.0, 0.0), vec3(0.52, 0.09, 0.05), vec3(0.88, 0.46, 0.25), vec3(0.86, 0.84, 0.82));
    return fireRamp(t, vec3(0.02, 0.0, 0.0), vec3(0.70, 0.07, 0.0), vec3(1.0, 0.44, 0.05), vec3(1.0, 0.95, 0.75));
}

void sceneTuning(int scene, out float heightBoost, out float turbulenceBoost, out float emberBoost, out float slenderness, out float flicker) {
    heightBoost = 1.0;
    turbulenceBoost = 1.0;
    emberBoost = 1.0;
    slenderness = 1.0;
    flicker = 1.0;

    if (scene == 1) {
        heightBoost = 1.22;
        turbulenceBoost = 1.18;
        emberBoost = 0.95;
        slenderness = 0.9;
        flicker = 1.18;
    } else if (scene == 2) {
        heightBoost = 0.9;
        turbulenceBoost = 0.75;
        emberBoost = 0.7;
        slenderness = 1.3;
        flicker = 0.85;
    } else if (scene == 3) {
        heightBoost = 1.05;
        turbulenceBoost = 1.45;
        emberBoost = 1.35;
        slenderness = 1.05;
        flicker = 1.32;
    }
}

float tongueLayer(vec2 uv, float time, float seedOffset, float count, float baseHeight, float sway, float edge) {
    float x = uv.x * count;
    float cell = floor(x);
    float local = fract(x) * 2.0 - 1.0;

    float seed = hash21(vec2(cell, seedOffset));
    float width = mix(0.26, 0.50, seed);
    float wobble = sin(time * (1.4 + seed * 2.4) + cell * (1.0 + seed * 0.45));
    float bend = (seed - 0.5) * 0.95 + wobble * sway;

    float profile = 1.0 - abs(local + bend * pow(uv.y + 0.04, 0.9) * 0.95);
    profile = smoothstep(width, width + edge, profile);

    float flicker = 0.82 + 0.24 * sin(time * (2.7 + seed * 3.3) + seed * 17.0);
    float reach = baseHeight * mix(0.62, 1.18, seed) * profile * flicker;

    return smoothstep(0.0, edge, reach - uv.y);
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    float aspect = iResolution.x / max(iResolution.y, 1.0);

    float speed = max(iSpeed, 0.05);
    float time = iTime * (0.45 + speed * 0.9);

    float heightCtrl = saturate(iFlameHeight / 100.0);
    float turbulenceCtrl = saturate(iTurbulence / 100.0);
    float intensityCtrl = saturate(iIntensity / 100.0);
    float emberCtrl = saturate(iEmberAmount / 100.0);

    float sceneHeight;
    float sceneTurbulence;
    float sceneEmber;
    float sceneSlenderness;
    float sceneFlicker;
    sceneTuning(iScene, sceneHeight, sceneTurbulence, sceneEmber, sceneSlenderness, sceneFlicker);

    vec3 audioBands = clamp(vec3(iAudioBass, iAudioMid, iAudioTreble), 0.0, 1.0);
    float audioPresence = smoothstep(0.025, 0.16, max(max(audioBands.x, audioBands.y), max(audioBands.z, iAudioLevel)));
    vec3 fallbackBands = vec3(
        0.35 + 0.12 * sin(time * 1.1),
        0.33 + 0.11 * sin(time * 1.3 + 1.5),
        0.31 + 0.10 * sin(time * 1.7 + 3.2)
    );
    vec3 bands = mix(fallbackBands, audioBands, audioPresence);

    float audioDrive = saturate(iAudioLevel * 1.15 + iAudioBeatPulse * 0.8 + iAudioSpectralFlux * 0.55);
    float drive = 0.28 + 0.46 * audioPresence * audioDrive;

    float turbulence = clamp(turbulenceCtrl * sceneTurbulence, 0.0, 1.6);

    vec2 fuv = uv;
    float flow = fbm(vec2((uv.x - 0.5) * aspect * (3.4 + turbulence * 4.8), uv.y * (2.6 + turbulence * 2.2) - time * (1.4 + 0.25 * sceneFlicker)));
    float curl = fbm(vec2((uv.x - 0.5) * aspect * (8.0 + turbulence * 8.0) + time * 0.35, uv.y * 5.8 - time * (2.5 + turbulence)));
    float ridged = ridge(fbm(vec2((uv.x - 0.5) * aspect * 6.5, uv.y * 8.0 - time * 3.2)));
    float xWarp = ((flow - 0.5) * (0.18 + turbulence * 0.34) + (curl - 0.5) * (0.12 + turbulence * 0.22) + (ridged - 0.5) * 0.1) * (0.25 + uv.y * 0.85);
    fuv.x += xWarp;

    float bassBand = (1.0 - smoothstep(0.22, 0.52, uv.x)) * bands.x;
    float midBand = (1.0 - abs(uv.x - 0.5) * 2.0) * bands.y;
    float trebleBand = smoothstep(0.48, 0.78, uv.x) * bands.z;
    float bandMix = saturate(bassBand + midBand + trebleBand) * (0.45 + 0.55 * audioPresence);

    float maxHeight = (0.38 + heightCtrl * 0.56 + drive * 0.16) * sceneHeight;
    maxHeight = clamp(maxHeight, 0.28, 1.05);

    float layer1 = tongueLayer(
        fuv,
        time * sceneFlicker,
        11.0 + bandMix * 5.0,
        mix(4.5, 7.0, turbulence) * sceneSlenderness,
        maxHeight * (0.74 + bandMix * 0.24),
        0.24 + turbulence * 0.30,
        0.07
    );

    float layer2 = tongueLayer(
        fuv + vec2(0.07 * sin(time * 0.9), 0.0),
        time * (1.25 + turbulence * 0.4),
        23.0 + bandMix * 7.0,
        mix(7.8, 11.8, turbulence) * sceneSlenderness,
        maxHeight * (0.66 + bandMix * 0.30),
        0.30 + turbulence * 0.36,
        0.055
    );

    float layer3 = tongueLayer(
        fuv + vec2(-0.05 * sin(time * 1.3 + 2.0), 0.0),
        time * (1.8 + turbulence * 0.6),
        37.0 + bandMix * 9.0,
        mix(10.5, 15.0, turbulence) * sceneSlenderness,
        maxHeight * (0.56 + bandMix * 0.24),
        0.38 + turbulence * 0.40,
        0.045
    );

    float flame = layer1 * 0.62 + layer2 * 0.50 + layer3 * 0.35;
    float cap = 1.0 - smoothstep(maxHeight - 0.08, maxHeight + 0.08, uv.y);
    flame *= cap;

    float breakup = ridge(fbm(vec2(fuv.x * 12.0 + time * 0.6, uv.y * 11.0 - time * 2.8)));
    flame = max(flame - breakup * (0.08 + uv.y * 0.14) * (0.5 + turbulence * 0.9), 0.0);

    float coreGradient = saturate((maxHeight - uv.y) / max(maxHeight, 0.001));
    float core = smoothstep(0.2, 0.92, flame) * pow(coreGradient, 0.55);
    float rim = smoothstep(0.05, 0.22, flame) - smoothstep(0.26, 0.65, flame);

    float temperature = saturate(flame * (0.55 + core * 0.6) + layer3 * 0.18 + drive * 0.2 + bandMix * 0.22);

    vec3 col = vec3(0.0);
    vec3 flameColor = paletteColor(temperature, iPalette);
    col += flameColor * flame * (0.62 + intensityCtrl * 1.18);
    col += paletteColor(saturate(temperature + 0.2), iPalette) * core * (0.16 + intensityCtrl * 0.42);
    col += paletteColor(0.82, iPalette) * rim * (0.03 + intensityCtrl * 0.08);

    float emberBedNoise = fbm(vec2(fuv.x * 7.0, time * 0.2));
    float emberBed = smoothstep(0.22, 0.0, uv.y) * (0.25 + emberBedNoise * 0.75) * (0.28 + intensityCtrl * 0.5);
    col += paletteColor(0.22, iPalette) * emberBed * 0.6;

    float emberDensity = emberCtrl * sceneEmber;
    emberDensity *= 0.85 + 0.3 * mix(0.25, saturate(iAudioBeatPulse), audioPresence);

    for (int layer = 0; layer < 3; layer++) {
        float fl = float(layer);
        float scale = 26.0 + fl * 18.0;
        vec2 puv = vec2(uv.x * scale, uv.y * (scale * 1.35));
        puv.y -= time * (1.9 + fl * 0.9);
        puv.x += sin(time * (0.7 + fl * 0.4) + fl * 2.0) * (1.2 + turbulence * 1.6);

        vec2 cell = floor(puv);
        vec2 local = fract(puv) - 0.5;
        float seed = hash21(cell + vec2(fl * 37.0, fl * 11.0));
        float threshold = emberDensity * (0.014 + fl * 0.007);

        if (seed < threshold) {
            vec2 jitter = (hash22(cell + vec2(17.0 + fl * 9.0, 29.0 + fl * 7.0)) - 0.5) * vec2(0.7, 0.5);
            float dist = length(local - jitter);
            float size = mix(0.13, 0.045, seed) * (1.0 - fl * 0.18);
            float ember = smoothstep(size, size * 0.22, dist);
            float trail = smoothstep(size * 2.4, 0.0, length(vec2((local.x - jitter.x) * 1.3, (local.y - jitter.y) * 0.45)));
            float cooling = 1.0 - saturate(uv.y * (0.8 + fl * 0.4));
            float emberFlicker = 0.72 + 0.28 * sin(time * (7.5 + fl * 1.8) + seed * 55.0);
            vec3 emberColor = paletteColor(0.6 + cooling * 0.35, iPalette);
            col += emberColor * (ember + trail * 0.3) * cooling * emberFlicker * (0.24 - fl * 0.05);
        }
    }

    float haze = fbm(vec2((uv.x - 0.5) * aspect * 2.8, uv.y * 3.6 - time * 0.4));
    col += paletteColor(0.12, iPalette) * haze * (0.03 + flame * 0.04) * (0.6 + intensityCtrl * 0.5);

    float vignette = 1.0 - 0.42 * length((uv - vec2(0.5, 0.28)) * vec2(1.2, 0.9));
    col *= max(vignette, 0.1);

    col = col / (1.0 + col * 0.55);
    col = pow(clamp(col, 0.0, 1.0), vec3(0.95));

    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
