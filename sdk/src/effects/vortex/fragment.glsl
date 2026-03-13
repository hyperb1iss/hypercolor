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
uniform float iTurbulence;
uniform float iIntensity;

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
        p = p * 2.07 + vec2(7.3, -4.1);
        amp *= 0.47;
    }
    return sum;
}

// Domain warping — fbm of fbm for organic distortion
vec2 domainWarp(vec2 p, float t, float strength) {
    vec2 q = vec2(
        fbm(p + vec2(1.7, 9.2) + t * 0.15),
        fbm(p + vec2(8.3, 2.8) - t * 0.12)
    );
    vec2 r = vec2(
        fbm(p + 4.0 * q + vec2(1.2, -3.4) + t * 0.08),
        fbm(p + 4.0 * q + vec2(6.7, 2.1) - t * 0.1)
    );
    return p + strength * r;
}

// ─── Palette ────────────────────────────────────────────────────────

vec3 triGradient(float t, vec3 a, vec3 b, vec3 c) {
    t = fract(t);
    if (t < 0.5) return mix(a, b, t * 2.0);
    return mix(b, c, (t - 0.5) * 2.0);
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return triGradient(t, vec3(0.88, 0.21, 1.00), vec3(0.50, 1.00, 0.92), vec3(1.00, 0.42, 0.76));
    if (id == 1) return triGradient(t, vec3(1.00, 0.00, 1.00), vec3(0.00, 1.00, 1.00), vec3(0.40, 0.00, 1.00));
    if (id == 2) return triGradient(t, vec3(0.48, 0.00, 0.72), vec3(1.00, 0.00, 0.42), vec3(1.00, 0.40, 0.00));
    if (id == 3) return triGradient(t, vec3(0.00, 0.90, 1.00), vec3(0.30, 0.69, 0.31), vec3(0.49, 0.30, 0.99));
    if (id == 4) return triGradient(t, vec3(0.55, 0.00, 0.00), vec3(1.00, 0.27, 0.00), vec3(1.00, 0.65, 0.00));
    if (id == 5) return triGradient(t, vec3(0.05, 0.28, 0.63), vec3(0.00, 0.90, 1.00), vec3(0.70, 0.90, 0.99));
    if (id == 6) return triGradient(t, vec3(0.00, 0.12, 0.25), vec3(0.00, 0.46, 0.85), vec3(0.22, 0.80, 0.80));
    return triGradient(t, vec3(1.00, 0.00, 0.67), vec3(0.00, 1.00, 0.80), vec3(0.67, 0.00, 1.00));
}

// ─── Main ───────────────────────────────────────────────────────────

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    float aspect = iResolution.x / iResolution.y;
    vec2 p = (uv - 0.5) * vec2(aspect, 1.0);

    float speed = max(iSpeed, 0.2);
    float arms = clamp(iArms, 2.0, 8.0);
    float twist = clamp(iTwist * 0.01, 0.0, 1.0);
    float depthCtrl = clamp(iDepth * 0.01, 0.0, 1.0);
    float turb = clamp(iTurbulence * 0.01, 0.0, 1.0);
    float intensity = clamp(iIntensity * 0.01, 0.0, 1.0);

    float time = iTime * (0.3 + speed * 0.14);

    // ── Domain warp — organic turbulence at low spatial frequency ──
    vec2 wp = domainWarp(p * 1.6, time, turb * 0.7);
    vec2 sp = mix(p, wp * 0.625, turb * 0.75);

    // ── Polar coordinates on warped space ──
    float radius = length(sp);
    float angle = atan(sp.y, sp.x);

    // Low-frequency noise for smooth arm distortion
    float noise1 = fbm(sp * 2.0 + vec2(time * 0.2, -time * 0.15)) - 0.5;
    float noise2 = fbm(sp * 3.0 - vec2(time * 0.25, time * 0.1)) - 0.5;

    // ── Differential rotation — inner parts spin much faster ──
    float diffRot = 1.0 / (radius * 0.8 + 0.15);
    float rotAngle = angle + time * diffRot;

    // ── Logarithmic spiral ──
    float logCoeff = mix(2.0, 14.0, twist);
    float logSpiral = rotAngle * arms
                    - log(radius + 0.001) * logCoeff
                    + noise1 * (0.6 + turb * 2.5);

    // ── Primary arms — ultra-wide soft Gaussian, LED-friendly ──
    float wave1 = sin(logSpiral);
    // Very soft body — wide enough to read on sparse LEDs
    float armBody = exp(-1.8 * (1.0 - wave1) * (1.0 - wave1));
    // Slightly brighter core within the soft body
    float armCore = exp(-5.0 * (1.0 - wave1) * (1.0 - wave1));

    // ── Secondary spiral — counter-rotating, shifted arm count ──
    float logSpiral2 = (angle - time * diffRot * 0.35) * (arms + 1.0)
                      - log(radius + 0.001) * logCoeff * 0.6
                      + noise2 * (0.8 + turb * 2.0);
    float wave2 = sin(logSpiral2);
    float arm2Body = exp(-2.2 * (1.0 - wave2) * (1.0 - wave2));

    // ── Plasma field — smooth turbulence fills the space between arms ──
    float plasma = fbm(sp * 2.5 + vec2(time * 0.35, time * 0.25));
    plasma += 0.5 * vnoise(sp * 4.0 - vec2(time * 0.4, time * 0.15));
    plasma *= 0.55;
    float interArm = 1.0 - armBody * 0.7;
    float plasmaField = plasma * interArm * turb;

    // ── Wide energy pulses from center — low frequency for LEDs ──
    float pulse1 = sin(radius * 6.0 - time * 4.0) * 0.5 + 0.5;
    pulse1 *= exp(-radius * 2.0);
    float pulse2 = sin(radius * 3.5 - time * 2.5 + 1.0) * 0.5 + 0.5;
    pulse2 *= exp(-radius * 1.5);

    // ── Breathing — whole vortex pulsates ──
    float breathe = 0.85 + 0.15 * sin(time * 1.8);
    float breathe2 = 0.9 + 0.1 * sin(time * 2.7 + 1.2);

    // ── Radial brightness ──
    float centerGlow = exp(-radius * mix(1.0, 4.5, depthCtrl));
    float edgeFade = 1.0 - smoothstep(0.3, 1.3, radius);
    float radialMask = mix(0.06, 1.0, centerGlow) * edgeFade;

    // Wide soft accretion glow at center
    float accretion = exp(-radius * radius * 20.0);
    float accretionPulse = 0.7 + 0.3 * sin(time * 3.2);

    // ── Color — distinct palette colors, minimal blending ──
    // Each layer gets its own palette position for vivid separation
    float colorT = logSpiral * 0.04 + time * 0.06;
    vec3 col1 = paletteColor(colorT, iPalette);
    vec3 col2 = paletteColor(colorT + 0.35, iPalette);
    vec3 col3 = paletteColor(colorT + 0.7, iPalette);

    // ── Compose — punchy layers, black gaps ──
    vec3 color = vec3(0.0);

    // Primary arms — vivid, full brightness on the arm
    float primaryStrength = armBody * 0.55 + armCore * 0.45;
    color += col1 * primaryStrength * radialMask * breathe * 1.3;

    // Secondary arms — distinct color, not blended with primary
    color += col2 * arm2Body * radialMask * 0.55 * (0.3 + depthCtrl * 0.7) * breathe2;

    // Plasma — only where turbulence is high, uses third color
    color += col3 * plasmaField * centerGlow * 0.5;

    // Energy pulses — additive pop, not wash
    float pulseBlend = pulse1 * 0.3 + pulse2 * 0.15;
    color += col3 * pulseBlend * intensity;

    // Core — hot, saturated center bloom
    float corePulse = 0.65 + 0.35 * sin(time * 3.2);
    color += col1 * accretion * corePulse * (0.5 + intensity * 0.8);

    // ── Saturation boost — push toward pure hues ──
    float lum = dot(color, vec3(0.2126, 0.7152, 0.0722));
    color = mix(vec3(lum), color, 1.3 + intensity * 0.3);

    // ── Vignette — gentle, don't crush brightness ──
    float vignette = smoothstep(1.6, 0.2, length(p));
    color *= 0.8 + 0.2 * vignette;

    // ── Intensity scales overall energy ──
    color *= 0.9 + intensity * 0.6;

    // ── Output — hard clamp only, no tone mapping compression ──
    fragColor = vec4(clamp(color, 0.0, 1.0), 1.0);
}
