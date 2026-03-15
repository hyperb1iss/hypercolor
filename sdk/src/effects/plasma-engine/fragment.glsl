#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform vec3 iBackgroundColor;
uniform vec3 iColor1;
uniform vec3 iColor2;
uniform vec3 iColor3;
uniform int iTheme;
uniform float iSpeed;
uniform float iBloom;
uniform float iSpread;
uniform float iDensity;

// IQ cosine palette — phase-separated channels never converge to white
vec3 cosPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return clamp(a + b * cos(6.283185 * (c * t + d)), 0.0, 1.0);
}

// Smooth cyclic interpolation for custom user colors
vec3 cyclicPalette(float t, vec3 c1, vec3 c2, vec3 c3) {
    float t3 = fract(t) * 3.0;
    vec3 color = mix(c1, c2, smoothstep(0.0, 1.0, t3));
    color = mix(color, c3, smoothstep(1.0, 2.0, t3));
    color = mix(color, c1, smoothstep(2.0, 3.0, t3));
    return color;
}

vec3 themedPalette(float t) {
    // Arcade: neon pink / electric blue / hot orange
    if (iTheme == 0) return cosPalette(t,
        vec3(0.45, 0.28, 0.38), vec3(0.55, 0.52, 0.58),
        vec3(1.0, 1.0, 1.0), vec3(0.00, 0.38, 0.68));
    // Aurora: emerald / sky blue / deep violet
    if (iTheme == 1) return cosPalette(t,
        vec3(0.28, 0.48, 0.48), vec3(0.42, 0.52, 0.52),
        vec3(1.0, 1.0, 1.0), vec3(0.58, 0.18, 0.42));
    // Custom: cyclic through user's 3 colors
    if (iTheme == 2) return cyclicPalette(t, iColor1, iColor2, iColor3);
    // Cyberpunk: hot magenta / electric cyan / deep purple
    if (iTheme == 3) return cosPalette(t,
        vec3(0.48, 0.28, 0.52), vec3(0.52, 0.48, 0.48),
        vec3(1.0, 1.0, 1.0), vec3(0.82, 0.18, 0.52));
    // Inferno: crimson / orange / deep magenta
    if (iTheme == 4) return cosPalette(t,
        vec3(0.52, 0.25, 0.18), vec3(0.48, 0.35, 0.42),
        vec3(1.0, 0.8, 0.6), vec3(0.00, 0.12, 0.32));
    // Oceanic: teal / blue / dark navy
    if (iTheme == 5) return cosPalette(t,
        vec3(0.12, 0.38, 0.52), vec3(0.18, 0.42, 0.48),
        vec3(1.0, 1.0, 0.8), vec3(0.58, 0.38, 0.18));
    // Poison: toxic green / jade / electric violet
    if (iTheme == 6) return cosPalette(t,
        vec3(0.22, 0.48, 0.35), vec3(0.38, 0.52, 0.55),
        vec3(1.0, 1.0, 1.0), vec3(0.65, 0.12, 0.48));
    // Tropical: amber / emerald / coral
    if (iTheme == 7) return cosPalette(t,
        vec3(0.48, 0.45, 0.28), vec3(0.52, 0.45, 0.42),
        vec3(1.0, 1.0, 0.8), vec3(0.00, 0.28, 0.58));
    // Fallback: classic rainbow
    return cosPalette(t,
        vec3(0.50, 0.50, 0.50), vec3(0.50, 0.50, 0.50),
        vec3(1.0, 1.0, 1.0), vec3(0.00, 0.33, 0.67));
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    float speed = max(iSpeed, 0.2);
    float glow = clamp(iBloom * 0.01, 0.0, 1.0);
    float spread = clamp(iSpread * 0.01, 0.0, 1.0);
    float density = clamp(iDensity * 0.01, 0.10, 1.0);
    float time = iTime * (0.20 + speed * 0.25);

    vec2 p = uv * 2.0 - 1.0;
    p.x *= iResolution.x / iResolution.y;

    // === DOMAIN WARP: two-pass feedback for organic flow ===
    vec2 warp1 = vec2(
        sin(p.y * 2.1 + time * 0.37) + cos(p.y * 0.8 - time * 0.23),
        cos(p.x * 1.9 - time * 0.31) + sin(p.x * 1.1 + time * 0.19)
    );
    vec2 warp2 = vec2(
        sin((p.y + warp1.y * 0.12) * 3.4 - time * 0.53),
        cos((p.x + warp1.x * 0.12) * 2.8 + time * 0.41)
    );
    float warpAmt = 0.15 + spread * 0.55;
    vec2 q = p * mix(1.8, 5.0, density) + (warp1 * 0.7 + warp2 * 0.3) * warpAmt;

    // === LAYER 1: Background swell — slow, large-scale structure ===
    float bg = sin(q.x * 0.6 + time * 0.19)
             + sin(q.y * 0.5 - time * 0.15)
             + sin((q.x - q.y) * 0.4 + time * 0.11);

    // === LAYER 2: Midground — radial + diagonal interference ===
    vec2 c1 = vec2(cos(time * 0.11) * 2.2, sin(time * 0.19) * 1.7);
    float mid = sin(length(q - c1) * 1.3 + time * 0.41)
              + sin((q.x + q.y) * 0.85 + time * 0.33)
              + sin(q.x * 1.3 - q.y * 0.7 + time * 0.47);

    // === LAYER 3: Foreground — fast shimmer with spiral arms ===
    vec2 c2 = vec2(sin(time * 0.23) * 1.6, cos(time * 0.29) * 1.9);
    float fg = sin(length(q + c2) * 2.1 - time * 0.61)
             + sin((q.x * 1.7 + q.y * 1.3) + time * 0.79);
    // Spiral term: rotational bands from orbiting center
    float angle = atan(q.y - c1.y, q.x - c1.x);
    fg += sin(angle * 3.0 + length(q - c1) * 1.2 + time * 0.53) * 0.7;

    // === COMBINE with depth-weighted layering ===
    float plasma = bg * 0.45 + mid * 0.35 + fg * 0.20;

    // === MULTIPLICATIVE CONTRAST: dark band zero-crossings ===
    float band = cos(q.x * 0.65 - q.y * 0.85 + time * 0.13);
    plasma *= 0.55 + 0.45 * band;

    // === SIN WRAP normalization — reveals interference structure ===
    plasma = 0.5 + 0.5 * sin(plasma * (1.5 + density * 1.0));

    // === PALETTE with cycling ===
    float shift = time * (0.02 + speed * 0.015);
    vec3 color = themedPalette(fract(plasma + shift));

    // === GLOW on peaks ===
    float peak = smoothstep(0.6, 0.95, plasma);
    color += color * peak * glow * 0.3;

    // === BACKGROUND in dark valleys only ===
    float dark = smoothstep(0.25, 0.0, plasma);
    color = mix(color, iBackgroundColor * 0.5, dark * 0.35);

    // === VIGNETTE ===
    float vig = smoothstep(1.5, 0.2, length(p));
    color *= 0.82 + 0.18 * vig;

    // === GAMMA for LED output ===
    color = pow(clamp(color, 0.0, 1.0), vec3(0.94));

    fragColor = vec4(color, 1.0);
}
