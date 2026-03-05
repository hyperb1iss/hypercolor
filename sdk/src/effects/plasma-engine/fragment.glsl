#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

// Controls
uniform float iSpeed;
uniform float iComplexity;
uniform float iDistortion;
uniform float iZoom;
uniform int iPalette;

// ── Palettes ───────────────────────────────────────────────────────────

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
    if (id == 1) return iqPalette(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0), vec3(0.0, 0.33, 0.67));
    if (id == 2) return iqPalette(t, vec3(0.5, 0.2, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 1.0), vec3(0.8, 0.1, 0.6));
    if (id == 3) return iqPalette(t, vec3(0.5, 0.2, 0.0), vec3(0.5, 0.4, 0.2), vec3(1.0, 0.7, 0.4), vec3(0.0, 0.15, 0.2));
    if (id == 4) return iqPalette(t, vec3(0.2, 0.5, 0.4), vec3(0.3, 0.4, 0.4), vec3(0.8, 0.7, 0.9), vec3(0.6, 0.3, 0.7));
    if (id == 5) return iqPalette(t, vec3(0.6, 0.4, 0.7), vec3(0.3, 0.3, 0.3), vec3(0.6, 0.8, 1.0), vec3(0.7, 0.3, 0.6));
    return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
}

// ── Plasma ─────────────────────────────────────────────────────────────

void main() {
    vec2 uv = (gl_FragCoord.xy - 0.5 * iResolution) / iResolution.y;
    float time = iTime * iSpeed * 0.25;
    float zoom = 1.0 + iZoom * 0.06;

    uv *= zoom;

    // 4-wave interference pattern (classic plasma)
    float waves = float(int(iComplexity * 0.06) + 2);

    float plasma = 0.0;

    // Wave 1: horizontal ripple
    plasma += sin(uv.x * 5.0 + time * 1.1);

    // Wave 2: vertical ripple
    plasma += sin(uv.y * 4.0 + time * 0.9);

    // Wave 3: diagonal wave
    plasma += sin((uv.x + uv.y) * 3.5 + time * 1.3);

    // Wave 4: radial wave
    float r = length(uv);
    plasma += sin(r * 6.0 - time * 1.5);

    // Additional complexity waves
    if (iComplexity > 30.0) {
        plasma += sin(uv.x * 3.0 * sin(time * 0.3) + uv.y * 4.0 * cos(time * 0.2)) * 0.8;
    }
    if (iComplexity > 60.0) {
        plasma += sin(length(uv - vec2(sin(time * 0.4), cos(time * 0.3))) * 8.0) * 0.6;
    }

    // Normalize
    plasma /= waves;

    // Distortion: warp the plasma field
    float distort = iDistortion * 0.008;
    float warpedPlasma = plasma;
    warpedPlasma += sin(plasma * 3.14159 + time) * distort;
    warpedPlasma += cos(plasma * 2.0 - time * 0.7) * distort * 0.7;

    // Map to 0-1 range
    float t_color = warpedPlasma * 0.5 + 0.5;

    // Color from palette
    vec3 col = paletteColor(t_color + time * 0.02, iPalette);

    // Brightness modulation — pulsing glow
    float brightness = 0.6 + 0.4 * sin(plasma * 3.14159);
    col *= brightness;

    // Slight glow in bright regions
    float glow = smoothstep(0.7, 1.0, brightness) * 0.3;
    col += paletteColor(t_color + 0.2, iPalette) * glow;

    // Gentle tonemapping
    col = col / (1.0 + col * 0.3);

    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
