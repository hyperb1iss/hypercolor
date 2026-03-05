#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

// Controls
uniform float iSpeed;
uniform float iIntensity;
uniform float iEmberDensity;
uniform float iHeatWarp;
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

// ── Blackbody-inspired color ───────────────────────────────────────────

vec3 blackbody(float t) {
    // Approximate blackbody radiation color
    // t: 0 = cold/dark, 1 = white hot
    vec3 col;
    col.r = smoothstep(0.0, 0.4, t);
    col.g = smoothstep(0.2, 0.7, t) * 0.8;
    col.b = smoothstep(0.6, 1.0, t) * 0.3;

    // Push toward orange-yellow at mid range
    col.r = pow(col.r, 0.6);
    col.g = pow(col.g, 1.2);

    return col;
}

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    // Ember (default) — blackbody
    if (id == 0) return blackbody(t);
    // Lava
    if (id == 1) {
        vec3 col = blackbody(t);
        col.r *= 1.2;
        col.b += smoothstep(0.5, 1.0, t) * 0.15;
        return col;
    }
    // SilkCircuit
    if (id == 2) return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
    // Solar
    if (id == 3) {
        vec3 col = blackbody(t * 0.8 + 0.2);
        col += vec3(0.1, 0.05, 0.0);
        return col;
    }
    // Phosphor green
    if (id == 4) return vec3(0.0, 1.0, 0.2) * smoothstep(0.0, 0.5, t) * (0.5 + t * 0.5);
    return blackbody(t);
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    float time = iTime * iSpeed * 0.2;

    // Heat distortion
    float heatWarp = iHeatWarp * 0.003;
    vec2 heatOffset = vec2(
        vnoise(vec2(uv.x * 4.0, uv.y * 6.0 + time * 2.0)) * heatWarp,
        vnoise(vec2(uv.x * 3.0 + 5.0, uv.y * 8.0 + time * 1.5)) * heatWarp
    );
    // Heat rises — stronger distortion at top
    heatOffset *= (0.3 + uv.y * 0.7);
    vec2 warped = uv + heatOffset;

    // ── Ember bed (bottom layer) ───────────────────────────────────────
    float emberNoise = fbm(warped * vec2(5.0, 3.0) + vec2(0.0, -time * 0.3), 5);
    float heatMap = fbm(warped * 3.0 + vec2(time * 0.1, -time * 0.2), 4);

    // Vertical gradient: hotter at bottom
    float verticalHeat = 1.0 - uv.y * 0.7;

    // Pulsing hotspots
    float pulse = sin(time * 0.8 + emberNoise * 6.28) * 0.5 + 0.5;
    float hotspots = smoothstep(0.4, 0.7, heatMap + pulse * 0.2) * verticalHeat;

    // Map to temperature
    float temperature = hotspots * iIntensity * 0.012;
    temperature = clamp(temperature, 0.0, 1.0);

    vec3 col = paletteColor(temperature, iPalette) * temperature;

    // ── Floating embers (particles rising) ─────────────────────────────
    float density = iEmberDensity * 0.35;
    for (int layer = 0; layer < 3; layer++) {
        float fl = float(layer);
        float particleScale = 30.0 + fl * 20.0;
        vec2 particleUV = warped * particleScale;

        // Particles rise
        particleUV.y -= time * (1.5 + fl * 0.8) * (1.0 + vnoise(vec2(fl * 10.0, time * 0.1)) * 0.5);
        // Slight horizontal drift
        particleUV.x += sin(time * 0.5 + fl * 2.0) * 2.0;

        vec2 cell = floor(particleUV);
        vec2 local = fract(particleUV) - 0.5;

        float rng = hash21(cell + fl * 100.0);
        if (rng < density * 0.01) {
            vec2 offset = (hash22(cell + fl * 77.0) - 0.5) * 0.6;
            float dist = length(local - offset);

            // Ember size varies
            float size = 0.03 + rng * 0.06;
            float ember = smoothstep(size, size * 0.2, dist);

            // Embers cool as they rise
            float lifeProgress = fract(time * 0.3 + rng);
            float cooling = 1.0 - lifeProgress * 0.6;

            // Flickering
            float flicker = 0.7 + 0.3 * sin(time * 8.0 + rng * 50.0);

            vec3 emberColor = paletteColor(cooling * 0.8 + 0.1, iPalette);
            col += emberColor * ember * flicker * cooling * (0.4 - fl * 0.1);
        }
    }

    // ── Ambient heat glow ──────────────────────────────────────────────
    float ambientHeat = fbm(warped * 2.0 + vec2(-time * 0.05), 3) * verticalHeat * 0.15;
    col += paletteColor(0.2, iPalette) * ambientHeat * iIntensity * 0.01;

    // Vignette — darker edges
    float vignette = 1.0 - 0.4 * length((uv - vec2(0.5, 0.3)) * vec2(1.2, 1.0));
    col *= max(vignette, 0.0);

    // Tonemapping
    col = col / (1.0 + col * 0.4);

    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
