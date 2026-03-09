#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform int iPalette;
uniform float iSpeed;
uniform float iArms;
uniform float iTwist;
uniform float iDepth;

// ─── Noise primitives ───────────────────────────────────────────────

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

float vnoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
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
    for (int i = 0; i < 4; i++) {
        sum += amp * vnoise(p);
        p = p * 2.04 + vec2(7.3, -4.1);
        amp *= 0.48;
    }
    return sum;
}

// ─── Palette ────────────────────────────────────────────────────────

vec3 triGradient(float t, vec3 a, vec3 b, vec3 c) {
    t = fract(t);
    if (t < 0.5) return mix(a, b, t * 2.0);
    return mix(b, c, (t - 0.5) * 2.0);
}

vec3 paletteColor(float t, int id) {
    // 0: SilkCircuit — purple, cyan, coral
    if (id == 0) return triGradient(t, vec3(0.88, 0.21, 1.00), vec3(0.50, 1.00, 0.92), vec3(1.00, 0.42, 0.76));
    // 1: Cyberpunk — magenta, cyan, deep blue
    if (id == 1) return triGradient(t, vec3(1.00, 0.00, 1.00), vec3(0.00, 1.00, 1.00), vec3(0.40, 0.00, 1.00));
    // 2: Synthwave — rich purple, hot pink, amber
    if (id == 2) return triGradient(t, vec3(0.48, 0.00, 0.72), vec3(1.00, 0.00, 0.42), vec3(1.00, 0.40, 0.00));
    // 3: Aurora — cyan, green, purple
    if (id == 3) return triGradient(t, vec3(0.00, 0.90, 1.00), vec3(0.30, 0.69, 0.31), vec3(0.49, 0.30, 0.99));
    // 4: Fire — dark red, orange-red, amber
    if (id == 4) return triGradient(t, vec3(0.55, 0.00, 0.00), vec3(1.00, 0.27, 0.00), vec3(1.00, 0.65, 0.00));
    // 5: Ice — deep blue, cyan, light blue
    if (id == 5) return triGradient(t, vec3(0.05, 0.28, 0.63), vec3(0.00, 0.90, 1.00), vec3(0.70, 0.90, 0.99));
    // 6: Ocean — navy, blue, teal
    if (id == 6) return triGradient(t, vec3(0.00, 0.12, 0.25), vec3(0.00, 0.46, 0.85), vec3(0.22, 0.80, 0.80));
    // 7: Neon Flux — hot pink, mint, purple
    return triGradient(t, vec3(1.00, 0.00, 0.67), vec3(0.00, 1.00, 0.80), vec3(0.67, 0.00, 1.00));
}

// ─── Main ───────────────────────────────────────────────────────────

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    vec2 p = (uv - 0.5) * vec2(iResolution.x / iResolution.y, 1.0);

    float speed = max(iSpeed, 0.2);
    float arms = clamp(iArms, 2.0, 6.0);
    float twist = clamp(iTwist * 0.01, 0.0, 1.0);
    float depthCtrl = clamp(iDepth * 0.01, 0.0, 1.0);

    // Time drives rotation — speed 4 ≈ one revolution every ~4.5s
    float time = iTime * (0.22 + speed * 0.12);

    // ── Polar coordinates ──
    float radius = length(p);
    float angle = atan(p.y, p.x);

    // ── Noise perturbation on spiral arms ──
    float noiseWarp = fbm(p * 2.8 + vec2(time * 0.3, -time * 0.2)) - 0.5;
    float fineNoise = vnoise(p * 5.6 - vec2(time * 0.4, time * 0.15)) - 0.5;

    // ── Differential rotation — inner parts spin faster ──
    float diffRotation = 1.0 / (radius + 0.30);
    float rotAngle = angle + time * diffRotation;

    // ── Logarithmic spiral ──
    // twist=0 → wide pinwheel (low log contribution)
    // twist=1 → tight corkscrew (high log contribution)
    float logSpiral = rotAngle * arms
                    - log(radius + 0.001) * mix(1.0, 12.0, twist)
                    + noiseWarp * (0.6 + twist * 0.8);

    // ── Spiral arm shape — smoothstep for soft LED-friendly edges ──
    float spiralWave = sin(logSpiral + fineNoise * 1.4);
    // Wide arms: smoothstep window gives broad, readable bands
    float armShape = smoothstep(-0.20, 0.60, spiralWave) * smoothstep(1.20, 0.50, spiralWave);

    // Secondary spiral layer offset for depth
    float spiral2 = sin(logSpiral * 0.5 + time * 0.7 + noiseWarp * 2.0);
    float arm2Shape = smoothstep(-0.10, 0.50, spiral2) * smoothstep(1.10, 0.40, spiral2);

    // ── Radial brightness falloff ──
    float centerGlow = exp(-radius * mix(1.6, 4.8, depthCtrl));
    float edgeFade = 1.0 - smoothstep(0.4, 1.2, radius);
    float radialMask = mix(0.08, 1.0, centerGlow) * edgeFade;

    // ── Color mapping ──
    // Primary color from spiral position
    float colorT = logSpiral * 0.08 + time * 0.06 + radius * 0.3;
    vec3 primaryColor = paletteColor(colorT, iPalette);

    // Shifted color along spiral for depth perception
    float colorT2 = colorT + 0.35 + noiseWarp * 0.2;
    vec3 secondaryColor = paletteColor(colorT2, iPalette);

    // Accent for the spiral core
    float colorT3 = colorT + 0.65 - fineNoise * 0.15;
    vec3 accentColor = paletteColor(colorT3, iPalette);

    // Combine arms
    float totalArm = armShape * 0.72 + arm2Shape * 0.28;
    vec3 armColor = mix(primaryColor, secondaryColor, arm2Shape * 0.6 + noiseWarp * 0.3);

    // Center brightening with accent
    float corePulse = exp(-radius * 6.0) * (0.5 + 0.5 * sin(time * 2.4 + radius * 8.0));
    vec3 coreColor = accentColor * corePulse * 0.35;

    // ── Compose final color ──
    // Near-black background
    vec3 bg = paletteColor(0.1 + time * 0.01, iPalette) * 0.02;

    vec3 color = bg;
    color += armColor * totalArm * radialMask;
    color += coreColor * radialMask;

    // Inter-arm glow — very subtle fill between arms
    float interGlow = smoothstep(0.0, 0.5, totalArm) * 0.12;
    color += primaryColor * interGlow * centerGlow * 0.3;

    // Vignette — darken edges
    float vignette = smoothstep(1.50, 0.20, length(p));
    color *= 0.70 + 0.30 * vignette;

    // ── Tone mapping ──
    color = max(color, vec3(0.0));
    color = pow(clamp(color, 0.0, 1.0), vec3(0.95));

    fragColor = vec4(color, 1.0);
}
