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
uniform int iBackground;

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
    f = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);

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
    if (id == 0) return fireRamp(t, vec3(0.015, 0.0, 0.0), vec3(0.82, 0.12, 0.0), vec3(1.0, 0.58, 0.02), vec3(1.0, 0.92, 0.45));
    if (id == 1) return fireRamp(t, vec3(0.02, 0.0, 0.0), vec3(0.92, 0.28, 0.0), vec3(1.0, 0.82, 0.28), vec3(0.82, 0.92, 1.0));
    if (id == 2) return fireRamp(t, vec3(0.0, 0.02, 0.02), vec3(0.0, 0.48, 0.52), vec3(0.18, 0.82, 0.68), vec3(0.92, 0.68, 0.22));
    if (id == 3) return fireRamp(t, vec3(0.01, 0.0, 0.04), vec3(0.32, 0.02, 0.72), vec3(0.85, 0.12, 0.68), vec3(1.0, 0.52, 0.42));
    if (id == 4) return fireRamp(t, vec3(0.025, 0.0, 0.01), vec3(0.78, 0.0, 0.32), vec3(1.0, 0.22, 0.48), vec3(1.0, 0.72, 0.28));
    if (id == 5) return fireRamp(t, vec3(0.0, 0.015, 0.005), vec3(0.04, 0.58, 0.12), vec3(0.32, 0.95, 0.28), vec3(0.78, 1.0, 0.52));
    if (id == 6) return fireRamp(t, vec3(0.0, 0.0, 0.03), vec3(0.04, 0.08, 0.72), vec3(0.28, 0.22, 0.98), vec3(0.82, 0.42, 1.0));
    if (id == 7) return fireRamp(t, vec3(0.015, 0.015, 0.0), vec3(0.48, 0.44, 0.0), vec3(0.95, 0.88, 0.08), vec3(0.52, 1.0, 0.58));
    return fireRamp(t, vec3(0.015, 0.0, 0.0), vec3(0.82, 0.12, 0.0), vec3(1.0, 0.58, 0.02), vec3(1.0, 0.92, 0.45));
}

vec3 bgColor(int id) {
    if (id == 1) return vec3(0.06, 0.05, 0.04);
    if (id == 2) return vec3(0.02, 0.02, 0.08);
    if (id == 3) return vec3(0.08, 0.01, 0.01);
    if (id == 4) return vec3(0.01, 0.06, 0.02);
    if (id == 5) return vec3(0.06, 0.03, 0.01);
    return vec3(0.0);
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

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    float aspect = iResolution.x / max(iResolution.y, 1.0);

    float speed = max(iSpeed, 0.05);
    float time = iTime * (0.45 + speed * 0.9);

    float heightCtrl = saturate(iFlameHeight / 100.0);
    float turbulenceCtrl = saturate(iTurbulence / 100.0);
    float intensityCtrl = saturate(iIntensity / 100.0);
    float emberCtrl = saturate(iEmberAmount / 100.0);

    float sceneHeight, sceneTurbulence, sceneEmber, sceneSlenderness, sceneFlicker;
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
    float maxHeight = (0.38 + heightCtrl * 0.56 + drive * 0.16) * sceneHeight;
    maxHeight = clamp(maxHeight, 0.28, 1.05);

    // ─── Domain warping — stronger at base to break up horizontal mass ───
    vec2 fuv = uv;
    float flow = fbm(vec2((uv.x - 0.5) * aspect * (3.4 + turbulence * 4.8), uv.y * (2.6 + turbulence * 2.2) - time * (1.4 + 0.25 * sceneFlicker)));
    float curl = fbm(vec2((uv.x - 0.5) * aspect * (6.5 + turbulence * 6.5) + time * 0.35, uv.y * 5.0 - time * (2.2 + turbulence)));
    float ridged = ridge(fbm(vec2((uv.x - 0.5) * aspect * 5.5, uv.y * 7.0 - time * 2.8)));
    float xWarp = ((flow - 0.5) * (0.28 + turbulence * 0.48) + (curl - 0.5) * (0.18 + turbulence * 0.30) + (ridged - 0.5) * 0.14) * (0.50 + uv.y * 0.70);
    fuv.x += xWarp;
    fuv.y += (flow - 0.5) * 0.05;

    float bassBand = (1.0 - smoothstep(0.22, 0.52, uv.x)) * bands.x;
    float midBand = (1.0 - abs(uv.x - 0.5) * 2.0) * bands.y;
    float trebleBand = smoothstep(0.48, 0.78, uv.x) * bands.z;
    float bandMix = saturate(bassBand + midBand + trebleBand) * (0.45 + 0.55 * audioPresence);

    // ─── Fire height field ───
    float xFreq = aspect * (3.5 * sceneSlenderness + turbulence * 1.5);
    float heightN1 = fbm(vec2(fuv.x * xFreq, time * 0.35 * sceneFlicker));
    float heightN2 = ridge(fbm(vec2(fuv.x * xFreq * 1.6 + time * 0.08, time * 0.5 * sceneFlicker + 3.0)));
    float heightN3 = fbm(vec2(fuv.x * xFreq * 2.3, time * 0.7 * sceneFlicker + 7.0));

    float localHeight = maxHeight * (0.30 + heightN1 * 0.35 + heightN2 * 0.30 + heightN3 * 0.12);
    localHeight += maxHeight * bandMix * 0.15;

    // ─── Fire body ───
    float edgeSoftness = 0.12 + turbulence * 0.08;
    float fireBody = smoothstep(localHeight + edgeSoftness * 0.3, localHeight - edgeSoftness, uv.y);

    // ─── Internal texture — vertically biased for flame-like streaks ───
    float flow1 = fbm(vec2(fuv.x * aspect * 3.5, uv.y * 6.0 - time * 1.8));
    float flow2 = fbm(vec2(fuv.x * aspect * 5.5, uv.y * 9.0 - time * 2.6 + 5.0));
    float flow3 = ridge(fbm(vec2(fuv.x * aspect * 5.0, uv.y * 10.0 - time * 3.2)));
    float internal = flow1 * 0.48 + flow2 * 0.30 + flow3 * 0.22;

    // More contrast — lower floor so dark pockets read as structure, not flat mass
    float flame = fireBody * (0.22 + internal * 0.78);

    // Breakup — stronger at base for more structure, gentler at tips
    float breakup = ridge(fbm(vec2(fuv.x * 10.0 + time * 0.5, uv.y * 9.0 - time * 2.4)));
    flame = max(flame - breakup * (0.07 + uv.y * 0.03) * (0.30 + turbulence * 0.50), 0.0);

    // ─── Coloring ───
    float coreGradient = saturate((localHeight - uv.y) / max(localHeight, 0.001));
    float core = smoothstep(0.25, 0.88, flame) * pow(coreGradient, 0.50);
    float rim = smoothstep(0.06, 0.24, flame) - smoothstep(0.28, 0.62, flame);
    float temperature = saturate(flame * (0.50 + core * 0.65) + drive * 0.2 + bandMix * 0.2);

    vec3 col = vec3(0.0);
    col += paletteColor(temperature, iPalette) * flame * (0.62 + intensityCtrl * 1.18);
    col += paletteColor(saturate(temperature + 0.2), iPalette) * core * (0.16 + intensityCtrl * 0.42);
    col += paletteColor(0.82, iPalette) * rim * (0.03 + intensityCtrl * 0.08);

    // ─── Ember bed — uses emberCtrl ───
    float emberBedNoise = fbm(vec2(fuv.x * 7.0, time * 0.2));
    float emberBed = smoothstep(0.18, 0.0, uv.y) * (0.25 + emberBedNoise * 0.75) * (0.28 + intensityCtrl * 0.5) * emberCtrl;
    col += paletteColor(0.22, iPalette) * emberBed * 0.6;

    // ─── Ember particles — visible and responsive ───
    float emberDensity = emberCtrl * sceneEmber;
    emberDensity *= 0.85 + 0.3 * mix(0.25, saturate(iAudioBeatPulse), audioPresence);

    for (int layer = 0; layer < 3; layer++) {
        float fl = float(layer);
        float scale = 26.0 + fl * 18.0;
        vec2 puv = vec2(uv.x * scale, uv.y * (scale * 1.35));
        puv.y -= time * (1.9 + fl * 0.9);
        puv.x += sin(time * (0.7 + fl * 0.4) + fl * 2.0) * (1.2 + turbulence * 1.6);

        vec2 pcell = floor(puv);
        vec2 local = fract(puv) - 0.5;
        float seed = hash21(pcell + vec2(fl * 37.0, fl * 11.0));
        float threshold = emberDensity * (0.025 + fl * 0.012);

        if (seed < threshold) {
            vec2 jitter = (hash22(pcell + vec2(17.0 + fl * 9.0, 29.0 + fl * 7.0)) - 0.5) * vec2(0.7, 0.5);
            float dist = length(local - jitter);
            float size = mix(0.15, 0.05, seed) * (1.0 - fl * 0.15);
            float ember = smoothstep(size, size * 0.18, dist);
            float trail = smoothstep(size * 2.4, 0.0, length(vec2((local.x - jitter.x) * 1.3, (local.y - jitter.y) * 0.45)));
            float cooling = 1.0 - saturate(uv.y * (0.8 + fl * 0.4));
            float emberFlicker = 0.72 + 0.28 * sin(time * (7.5 + fl * 1.8) + seed * 55.0);
            vec3 emberColor = paletteColor(0.6 + cooling * 0.35, iPalette);
            col += emberColor * (ember + trail * 0.3) * cooling * emberFlicker * (0.38 - fl * 0.06);
        }
    }

    // Haze
    float haze = fbm(vec2((uv.x - 0.5) * aspect * 2.8, uv.y * 3.6 - time * 0.4));
    col += paletteColor(0.12, iPalette) * haze * (0.03 + flame * 0.04) * (0.6 + intensityCtrl * 0.5);

    // Vignette + tone mapping
    float vignette = 1.0 - 0.42 * length((uv - vec2(0.5, 0.28)) * vec2(1.2, 0.9));
    col *= max(vignette, 0.1);
    col = col / (1.0 + col * 0.55);
    col = pow(clamp(col, 0.0, 1.0), vec3(0.95));

    // Background applied after tone mapping — not affected by vignette
    col = max(col, bgColor(iBackground));

    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
