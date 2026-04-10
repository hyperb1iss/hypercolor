#version 300 es
precision highp float;

out vec4 fragColor;

uniform vec2 iResolution;
uniform float iTime;

uniform float iSpeed;
uniform float iColorIntensity;
uniform int iSegments;
uniform float iTwist;
uniform int iColorMode;
uniform float iWarp;
uniform float iPulse;
uniform int iStyle;

const float TAU = 6.28318530718;
const float BASE_COLOR_SHIFT = 1.0;
const float BASE_ABERRATION = 0.2;

vec3 cosPal(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(TAU * (c * t + d));
}

vec3 palette(float t, int mode) {
    t = fract(t) + sin(iTime * 0.17) * 0.03;

    if (mode == 0) {
        // Amethyst — hot pink, magenta, violet, indigo crystal
        return cosPal(t, vec3(0.60, 0.15, 0.58), vec3(0.35, 0.12, 0.33),
                      vec3(0.7, 0.5, 0.8), vec3(0.00, 0.55, 0.50));
    }
    if (mode == 1) {
        // Deep Sea — dark navy, bioluminescent teal, deep blue-green
        return cosPal(t, vec3(0.10, 0.32, 0.50), vec3(0.08, 0.24, 0.35),
                      vec3(0.8, 0.7, 0.6), vec3(0.30, 0.20, 0.25));
    }
    if (mode == 2) {
        // Electric — blue-white lightning arcs, deep blue valleys
        return cosPal(t, vec3(0.35, 0.45, 0.72), vec3(0.40, 0.38, 0.28),
                      vec3(1.5, 1.0, 0.6), vec3(0.05, 0.10, 0.30));
    }
    if (mode == 3) {
        // Monochrome — true greyscale with smooth contrast
        float v = 0.5 + 0.45 * cos(TAU * t);
        return vec3(v);
    }
    if (mode == 4) {
        // Neon — hot pink, electric blue, acid green, neon yellow
        return cosPal(t, vec3(0.50, 0.50, 0.50), vec3(0.50, 0.50, 0.50),
                      vec3(1.0, 1.0, 1.0), vec3(0.88, 0.15, 0.52));
    }
    if (mode == 5) {
        // Rainbow — full vivid spectrum sweep
        return cosPal(t, vec3(0.50, 0.50, 0.50), vec3(0.50, 0.50, 0.50),
                      vec3(1.0, 1.0, 1.0), vec3(0.00, 0.33, 0.67));
    }
    if (mode == 6) {
        // Sunset — bright orange, dusky rose, deep purple horizon
        return cosPal(t, vec3(0.55, 0.30, 0.20), vec3(0.45, 0.28, 0.25),
                      vec3(0.7, 0.5, 0.8), vec3(0.00, 0.10, 0.55));
    }
    if (mode == 7) {
        // Toxic — vivid acid green, dark teal, purple undertones
        return cosPal(t, vec3(0.25, 0.50, 0.15), vec3(0.20, 0.40, 0.15),
                      vec3(0.8, 0.7, 0.9), vec3(0.35, 0.05, 0.60));
    }
    // Vaporwave — dusty pink, lavender, soft teal, retro pastel
    return cosPal(t, vec3(0.60, 0.45, 0.60), vec3(0.30, 0.25, 0.30),
                  vec3(0.8, 0.8, 0.8), vec3(0.90, 0.40, 0.55));
}

void main() {
    vec2 uv = (gl_FragCoord.xy - 0.5 * iResolution.xy) / iResolution.y;

    float speed;
    if (iSpeed <= 10.0) {
        float speedT = clamp((iSpeed - 1.0) / 9.0, 0.0, 1.0);
        speed = 0.05 + pow(speedT, 1.2) * 0.5;
    } else {
        float overdriveT = clamp((iSpeed - 10.0) / 10.0, 0.0, 1.0);
        speed = 0.55 + pow(overdriveT, 1.1) * 0.85;
    }
    float time = iTime * speed;

    float warp = clamp(iWarp / 100.0, 0.0, 1.0);
    uv += vec2(
        sin((uv.y + time * 0.5) * 6.0) * warp * 0.065,
        cos((uv.x - time * 0.4) * 6.0) * warp * 0.065
    );

    float radius = length(uv) + 1e-6;
    float angle = atan(uv.y, uv.x);

    float segments = max(3.0, float(iSegments));
    float sector = TAU / segments;
    angle = mod(angle + sector * 0.5, sector) - sector * 0.5;
    angle = abs(angle);

    float twistBase = clamp(iTwist / 100.0, 0.0, 1.0);
    float twist = twistBase * (0.6 + 0.4 * sin(time * 0.5));
    float twistPhase = time * twistBase * 0.4;
    float twistedAngle = angle + radius * twist * 2.0 + twistPhase;

    float waveAngular = sin(twistedAngle * (6.0 + segments * 0.5) - time * 1.6);
    float waveRadial = sin(radius * (18.0 + segments * 0.8) - time * 1.2);
    float pattern = 0.5 + 0.5 * (0.6 * waveAngular + 0.4 * waveRadial);

    float pulseControl = clamp(iPulse / 100.0, 0.0, 1.0);
    float hue = fract(twistedAngle / sector + radius * 0.25 + time * 0.06);
    hue += BASE_COLOR_SHIFT * 0.1 * sin(time * (0.6 + 0.4 * pulseControl) + radius * 4.0);

    float pulse = 1.0 + 0.65 * pulseControl * sin(time * 2.5 + radius * 6.0);
    float shimmer = 0.92 + 0.08 * sin(twistedAngle * 3.0 - time * (1.8 + pulseControl));
    vec3 base = palette(hue, iColorMode);

    float intensity = clamp(iColorIntensity / 100.0, 0.2, 2.2);
    vec3 color = base * pattern * intensity * pulse * shimmer;

    float vignette = smoothstep(1.3, 0.2, radius);
    color *= vignette;

    // Hue-preserving soft clamp — prevent white-out at high intensity/pulse
    float peak = max(color.r, max(color.g, color.b));
    if (peak > 1.0) color /= peak;

    if (iStyle == 0) {
        // Glitch — radial corruption bands with channel rotation per depth ring
        float sweep = sin(radius * 16.0 - time * 4.5);
        float band = smoothstep(0.5, 0.9, sweep);
        float rotation = mod(floor(radius * 8.0 + time * 2.0), 3.0);
        vec3 shifted = rotation < 1.0 ? color.gbr : rotation < 2.0 ? color.brg : color.rgb;
        color = mix(color, shifted * 1.1, band);
        float edge = smoothstep(0.8, 0.95, sweep) - smoothstep(0.95, 1.0, sweep);
        color += edge * 0.2;
    } else if (iStyle == 1) {
        // Grain — crystalline texture, heavy at segment edges
        float grain = fract(sin(dot(gl_FragCoord.xy, vec2(12.9898, 78.233))) * 43758.5453);
        float segEdge = 1.0 - abs(sin(twistedAngle * segments * 0.5));
        float depthFade = smoothstep(0.0, 0.8, radius);
        float grainStrength = 0.04 + 0.10 * pow(segEdge, 3.0) + 0.04 * depthFade;
        color += (grain - 0.5) * grainStrength;
    } else if (iStyle == 2) {
        // Holo — prismatic diffraction following tunnel geometry
        float holo = twistedAngle * 2.0 + radius * 5.0 + time * 2.5;
        vec3 prism = 0.5 + 0.5 * cos(holo + vec3(0.0, 2.094, 4.189));
        color += color * prism * 0.25;
    }

    float aberration = BASE_ABERRATION * 0.003;
    vec3 rShift = color;
    vec3 gShift = color;
    vec3 bShift = color;
    rShift.r = clamp(color.r + aberration * 0.8, 0.0, 1.0);
    bShift.b = clamp(color.b + aberration * 0.8, 0.0, 1.0);
    color = vec3(rShift.r, gShift.g, bShift.b);

    fragColor = vec4(color, 1.0);
}
