#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform int iPalette;
uniform float iSpeed;
uniform float iDepth;
uniform float iDistortion;
uniform float iRingCount;

// ─── Constants ───────────────────────────────────────────────────────

const float TAU = 6.2831853;
const float PI  = 3.1415927;

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
    for (int i = 0; i < 5; i++) {
        sum += amp * vnoise(p);
        p = p * 2.04 + vec2(7.3, -4.1);
        amp *= 0.46;
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
    // 0: Event Horizon — deep blue walls, cyan energy, white-hot rings
    if (id == 0) return triGradient(t, vec3(0.00, 0.13, 0.67), vec3(0.00, 0.90, 1.00), vec3(0.85, 0.93, 1.00));
    // 1: Void Gate — purple walls, magenta energy, pink rings
    if (id == 1) return triGradient(t, vec3(0.40, 0.00, 0.67), vec3(1.00, 0.00, 0.67), vec3(1.00, 0.40, 0.67));
    // 2: Quantum — teal walls, electric green energy, cyan rings
    if (id == 2) return triGradient(t, vec3(0.00, 0.53, 0.53), vec3(0.00, 1.00, 0.40), vec3(0.00, 1.00, 0.80));
    // 3: Abyssal — near-black walls, deep red energy, orange rings
    if (id == 3) return triGradient(t, vec3(0.08, 0.02, 0.02), vec3(0.53, 0.00, 0.00), vec3(1.00, 0.27, 0.00));
    // 4: Solar Flare — dark amber walls, orange energy, yellow-white rings
    if (id == 4) return triGradient(t, vec3(0.30, 0.12, 0.00), vec3(1.00, 0.40, 0.00), vec3(1.00, 0.80, 0.27));
    // 5: Spectral — indigo base, spectrum shifts with depth (hue rotation)
    float hue = fract(t * 1.5);
    float s = 0.9;
    float v = 0.85 + 0.15 * sin(t * TAU);
    // HSV to RGB
    vec3 rgb = clamp(abs(mod(hue * 6.0 + vec3(0.0, 4.0, 2.0), 6.0) - 3.0) - 1.0, 0.0, 1.0);
    return v * mix(vec3(1.0), rgb, s);
}

// ─── Main ───────────────────────────────────────────────────────────

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    vec2 p = (uv - 0.5) * vec2(iResolution.x / iResolution.y, 1.0);

    // ── Normalize controls ──
    float speed     = max(iSpeed, 0.2);
    float depthCtrl = clamp(iDepth * 0.01, 0.0, 1.0);
    float distort   = clamp(iDistortion * 0.01, 0.0, 1.0);
    float ringFreq  = 2.0 + clamp(iRingCount * 0.01, 0.0, 1.0) * 18.0;

    float time = iTime * (0.15 + speed * 0.18);

    // ── Polar coordinates ──
    float radius = length(p);
    float angle  = atan(p.y, p.x);

    // ── Tunnel inversion — the core trick ──
    // Depth via 1/r: near the center = infinitely far away
    float tunnelDepth = 1.0 / (radius + 0.001);

    // Scale depth feel — higher depthCtrl = more aggressive perspective
    float depthScale = mix(0.4, 2.0, depthCtrl);
    float tDepth = tunnelDepth * depthScale;

    // ── Tunnel UV coordinates ──
    float tunnelAngle = angle / TAU;

    // Twist increases with depth — spiral inward
    float twist = mix(0.3, 1.8, distort);
    float rotSpeed = 0.08 + speed * 0.03;

    vec2 tUV = vec2(
        tunnelAngle + tDepth * twist + time * rotSpeed,
        tDepth + time * speed * 0.4
    );

    // ── Distortion — noise warp on tunnel coordinates ──
    float warpAmount = distort * 0.6;
    vec2 warpOffset = vec2(
        fbm(tUV * vec2(2.0, 1.5) + vec2(time * 0.2, 0.0)) - 0.5,
        fbm(tUV * vec2(1.5, 2.0) + vec2(0.0, time * 0.15)) - 0.5
    );
    vec2 warpedUV = tUV + warpOffset * warpAmount;

    // ── Layer 1: Tunnel wall texture (FBM noise) ──
    float wallNoise = fbm(warpedUV * vec2(3.0, 8.0));
    float wallDetail = vnoise(warpedUV * vec2(6.0, 16.0) + vec2(time * 0.1));
    float wall = wallNoise * 0.7 + wallDetail * 0.3;

    // ── Layer 2: Energy bands / concentric rings ──
    float ringPhase = warpedUV.y * ringFreq + wallNoise * 2.5;
    float rings = sin(ringPhase);
    // Shape rings into sharp bands with soft edges (LED-friendly)
    float ringShape = smoothstep(0.2, 0.6, rings) * smoothstep(1.0, 0.7, rings);

    // Secondary ring layer — offset frequency for complexity
    float rings2 = sin(ringPhase * 0.37 + time * 1.2 + wallNoise * 1.8);
    float ring2Shape = smoothstep(0.3, 0.7, rings2) * smoothstep(1.0, 0.65, rings2);

    // ── Layer 3: Spiral energy streaks ──
    float spiralPhase = tunnelAngle * 6.0 + tDepth * twist * 2.0 + time * 0.5;
    float spiral = sin(spiralPhase + wallNoise * 3.0);
    float spiralShape = smoothstep(0.4, 0.8, spiral) * smoothstep(1.0, 0.65, spiral);

    // ── Combine pattern layers ──
    float pattern = wall * 0.25 + ringShape * 0.45 + ring2Shape * 0.15 + spiralShape * 0.15;

    // ── Radial masking — entrance glow and depth fog ──
    // Entrance rim: bright at the outer edge (close to viewer)
    float entranceGlow = smoothstep(0.05, 0.25, radius) * smoothstep(1.4, 0.5, radius);

    // Void center: fade toward the center (the abyss)
    float voidFade = smoothstep(0.0, 0.12, radius);

    // Or bright destination core when depth is high
    float coreLight = exp(-radius * mix(8.0, 3.0, depthCtrl)) * depthCtrl * 0.6;

    // Edge ring energy — bright ring at tunnel entrance
    float rimEnergy = exp(-pow((radius - 0.55) * 3.0, 2.0)) * 0.4;

    // ── Color mapping ──
    // Color varies with depth + angle for visual richness
    float colorT = warpedUV.y * 0.06 + tunnelAngle * 0.3 + time * 0.04 + wallNoise * 0.2;

    // Wall color — sampled from deeper in the palette
    vec3 wallColor = paletteColor(colorT, iPalette);

    // Ring energy color — brighter, shifted hue
    vec3 ringColor = paletteColor(colorT + 0.33, iPalette);

    // Core / destination color — white-shifted for intensity
    vec3 coreColor = paletteColor(colorT + 0.15, iPalette);
    coreColor = mix(coreColor, vec3(1.0), 0.3);

    // ── Compose final color ──
    // Dark base — near black
    vec3 color = paletteColor(0.0, iPalette) * 0.015;

    // Wall texture contribution
    color += wallColor * wall * 0.35 * entranceGlow * voidFade;

    // Ring energy — the main visual punch
    color += ringColor * (ringShape * 0.7 + ring2Shape * 0.3) * entranceGlow * voidFade;

    // Spiral streaks
    color += wallColor * spiralShape * 0.25 * entranceGlow * voidFade;

    // Entrance rim glow
    color += ringColor * rimEnergy;

    // Central destination light
    color += coreColor * coreLight;

    // ── Depth-based intensity modulation ──
    // Things deeper in the tunnel get a slight color shift
    float depthShift = smoothstep(0.5, 0.0, radius);
    color = mix(color, color * paletteColor(colorT + 0.5, iPalette) * 1.5, depthShift * 0.2);

    // ── Pulsing energy — subtle throb in the tunnel ──
    float pulse = 0.9 + 0.1 * sin(time * 3.0 + tDepth * 2.0);
    color *= pulse;

    // ── Vignette — darken extreme edges ──
    float vignette = smoothstep(1.5, 0.3, length(p));
    color *= 0.75 + 0.25 * vignette;

    // ── Tone mapping — prevent blowout ──
    color = max(color, vec3(0.0));
    // Soft clamp with slight HDR compression
    color = color / (color + vec3(0.15));
    color = pow(clamp(color * 1.15, 0.0, 1.0), vec3(0.92));

    fragColor = vec4(color, 1.0);
}
