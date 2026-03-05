#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

// Controls
uniform float iSpeed;
uniform float iIntensity;
uniform float iWarpStrength;
uniform float iStarBrightness;
uniform float iCurtainHeight;
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
    for (int i = 0; i < 8; i++) {
        if (i >= octaves) break;
        sum += amp * vnoise(p * freq);
        freq *= 2.0;
        amp *= 0.5;
    }
    return sum;
}

vec2 domainWarp(vec2 p, float strength, float scale) {
    float n1 = vnoise(p * scale);
    float n2 = vnoise(p * scale + vec2(5.2, 1.3));
    return p + vec2(n1, n2) * strength;
}

// ── Palettes ───────────────────────────────────────────────────────────

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    // Aurora (default)
    if (id == 0) return iqPalette(t, vec3(0.2, 0.5, 0.4), vec3(0.3, 0.4, 0.4), vec3(0.8, 0.7, 0.9), vec3(0.6, 0.3, 0.7));
    // SilkCircuit
    if (id == 1) return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
    // Cyberpunk
    if (id == 2) return iqPalette(t, vec3(0.5, 0.2, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 1.0), vec3(0.8, 0.1, 0.6));
    // Sunset
    if (id == 3) return iqPalette(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 1.0), vec3(0.0, 0.1, 0.2));
    // Ice
    if (id == 4) return iqPalette(t, vec3(0.5, 0.6, 0.8), vec3(0.2, 0.3, 0.2), vec3(0.6, 0.8, 1.0), vec3(0.0, 0.1, 0.3));
    // Fire
    if (id == 5) return iqPalette(t, vec3(0.5, 0.2, 0.0), vec3(0.5, 0.4, 0.2), vec3(1.0, 0.7, 0.4), vec3(0.0, 0.15, 0.2));
    // Vaporwave
    if (id == 6) return iqPalette(t, vec3(0.6, 0.4, 0.7), vec3(0.3, 0.3, 0.3), vec3(0.6, 0.8, 1.0), vec3(0.7, 0.3, 0.6));
    // Phosphor
    if (id == 7) return iqPalette(t, vec3(0.0, 0.3, 0.0), vec3(0.0, 0.5, 0.0), vec3(1.0, 1.0, 1.0), vec3(0.0, 0.0, 0.0));
    return iqPalette(t, vec3(0.2, 0.5, 0.4), vec3(0.3, 0.4, 0.4), vec3(0.8, 0.7, 0.9), vec3(0.6, 0.3, 0.7));
}

// ── Stars ──────────────────────────────────────────────────────────────

float starField(vec2 uv, float time) {
    // Grid of potential star positions
    vec2 cell = floor(uv * 120.0);
    vec2 local = fract(uv * 120.0) - 0.5;

    // Deterministic placement per cell
    float rng = hash21(cell);
    if (rng > 0.06) return 0.0; // sparse

    // Star position within cell
    vec2 starPos = hash22(cell) - 0.5;
    float dist = length(local - starPos * 0.6);

    // Point brightness with twinkle
    float twinkle = 0.6 + 0.4 * sin(time * (1.5 + rng * 4.0) + rng * 6.28);
    float brightness = smoothstep(0.04, 0.0, dist) * twinkle;

    // Some stars are brighter/dimmer
    brightness *= 0.4 + rng * 10.0;

    return clamp(brightness, 0.0, 1.0);
}

// ── Aurora ─────────────────────────────────────────────────────────────

vec3 auroraCurtain(vec2 uv, float time, float layerIdx, float totalLayers) {
    float fi = layerIdx;
    float layerOffset = fi / totalLayers;

    // Slow horizontal drift per layer
    float drift = time * (0.08 + fi * 0.02);

    // Domain warp for organic flowing shapes
    float warpAmt = iWarpStrength * 0.006;
    vec2 warped = domainWarp(
        vec2(uv.x + drift, uv.y * 0.5 + fi * 3.7),
        warpAmt * (1.0 + fi * 0.3),
        1.5 + fi * 0.4
    );

    // Curtain shape: tall vertical bands that sway
    float curtainX = fbm(vec2(warped.x * 2.5 + fi * 7.3, time * 0.15 + fi * 2.1), 5);

    // Vertical envelope — curtain hangs from above
    float curtainBase = iCurtainHeight * 0.01;
    float curtainTop = curtainBase + 0.15 + curtainX * 0.25;
    float curtainBottom = curtainBase - 0.1 - curtainX * 0.15;

    // Soft vertical falloff (bright at top, fading down like real aurora)
    float vertFade = smoothstep(curtainBottom, curtainBase, uv.y)
                   * smoothstep(curtainTop + 0.15, curtainTop - 0.05, uv.y);

    // Fine vertical structure (the characteristic "rays")
    float rays = fbm(vec2(warped.x * 12.0 + fi * 5.0, uv.y * 8.0 + time * 0.3), 4);
    rays = 0.4 + rays * 0.6;

    // Horizontal brightness variation
    float horizBright = fbm(vec2(warped.x * 4.0 + drift * 2.0, fi * 11.0), 3);
    horizBright = 0.3 + horizBright * 0.7;

    // Color — shift along the palette per layer and position
    float colorT = layerOffset + warped.x * 0.15 + time * 0.02;
    vec3 color = paletteColor(colorT, iPalette);

    // Combine
    float intensity = vertFade * rays * horizBright * iIntensity * 0.014;

    // Deeper layers are dimmer
    intensity *= 1.0 - fi * 0.12;

    return color * intensity;
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    float time = iTime * iSpeed * 0.3;

    // Night sky gradient — very dark
    vec3 skyLow = vec3(0.0, 0.005, 0.015);
    vec3 skyHigh = vec3(0.005, 0.015, 0.04);
    vec3 col = mix(skyLow, skyHigh, uv.y);

    // Stars (behind aurora)
    float stars = starField(uv, iTime) * iStarBrightness * 0.012;
    col += vec3(0.8, 0.85, 1.0) * stars;

    // Aurora curtain layers
    int layers = 5;
    float totalLayers = float(layers);
    for (int i = 0; i < 5; i++) {
        col += auroraCurtain(uv, time, float(i), totalLayers);
    }

    // Subtle ground reflection at bottom
    float reflection = smoothstep(0.15, 0.0, uv.y);
    vec3 reflected = auroraCurtain(vec2(uv.x, 0.3), time, 0.0, totalLayers) * 0.3;
    col += reflected * reflection;

    // Tonemapping — gentle S-curve
    col = col / (1.0 + col * 0.6);
    col = pow(col, vec3(0.95)); // slight gamma lift

    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
