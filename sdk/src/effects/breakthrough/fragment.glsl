#version 300 es
precision highp float;

out vec4 fragColor;

uniform vec2 iResolution;
uniform float iTime;

uniform float iSpeed;
uniform float iFlow;
uniform int iSegments;
uniform float iTwist;
uniform int iColorMode;
uniform float iWarp;
uniform float iPulse;
uniform int iStyle;

const float TAU = 6.28318530718;
const float BASE_COLOR_SHIFT = 1.0;
const float BASE_ABERRATION = 0.2;

float saturate(float x) {
    return clamp(x, 0.0, 1.0);
}

mat2 rot2(float a) {
    float c = cos(a);
    float s = sin(a);
    return mat2(c, -s, s, c);
}

vec3 cosPal(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(TAU * (c * t + d));
}

vec3 limitWhiteness(vec3 color, float maxRatio) {
    float peak = max(color.r, max(color.g, color.b));
    if (peak <= 0.00001) {
        return color;
    }

    float floor = min(color.r, min(color.g, color.b));
    float ratio = floor / peak;
    if (ratio <= maxRatio) {
        return color;
    }

    float targetFloor = peak * maxRatio;
    float spread = peak - floor;
    if (spread <= 0.00001) {
        return color * 0.9;
    }

    float remap = (peak - targetFloor) / spread;
    return max((color - vec3(floor)) * remap + vec3(targetFloor), 0.0);
}

vec3 liftMids(vec3 color, float amount) {
    vec3 safe = max(color, vec3(0.0));
    vec3 lifted = pow(safe, vec3(0.86));
    return mix(color, lifted, clamp(amount, 0.0, 1.0));
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
    float speedControl = clamp((iSpeed - 1.0) / 19.0, 0.0, 1.0);
    float flowControl = clamp(iFlow / 100.0, 0.0, 1.0);

    float speed;
    if (iSpeed <= 10.0) {
        float speedT = clamp((iSpeed - 1.0) / 9.0, 0.0, 1.0);
        speed = 0.05 + pow(speedT, 1.2) * 0.5;
    } else {
        float overdriveT = clamp((iSpeed - 10.0) / 10.0, 0.0, 1.0);
        speed = 0.55 + pow(overdriveT, 1.1) * 0.85;
    }
    float time = iTime * speed;
    float pulseControl = clamp(iPulse / 100.0, 0.0, 1.0);
    float warp = clamp(iWarp / 100.0, 0.0, 1.0);

    float breath = 1.0 + 0.09 * pulseControl * sin(time * 2.2);
    uv *= breath;
    uv += vec2(sin(time * 0.37), cos(time * 0.29)) * (0.018 + 0.045 * pulseControl);

    vec2 preWarp = uv;
    float preRadius = length(preWarp) + 1e-6;
    float preAngle = atan(preWarp.y, preWarp.x);
    vec2 warped = preWarp;
    warped += vec2(
        sin((preWarp.y + time * 0.55) * (6.0 + warp * 6.0)),
        cos((preWarp.x - time * 0.42) * (6.0 + warp * 5.0))
    ) * warp * 0.05;
    warped += vec2(
        cos(preRadius * 12.0 - time * 1.7 + preAngle * 2.0),
        sin(preAngle * (5.0 + warp * 8.0) + time * 1.2)
    ) * warp * 0.035;
    uv = mix(preWarp, warped, 0.55 + 0.30 * warp);
    uv = rot2(sin(time * 0.3) * warp * 0.12) * uv;

    // Log-depth shells create visible outward travel through the tunnel instead of only local warp.
    float radius = length(uv) + 1e-6;
    vec2 radialDir = uv / radius;
    float depth = -log(radius + 0.06);
    float flowRate = 0.45 + flowControl * 1.25 + speedControl * 1.0 + pulseControl * 0.8 + warp * 0.35;
    float shellPhase = fract(depth * (2.4 + warp * 2.4) + time * flowRate);
    float shell = exp(-16.0 * abs(shellPhase - 0.5));
    float shellPhaseAlt = fract(depth * (3.6 + warp * 3.1) + time * (flowRate * 1.22 + 0.35));
    float shellAlt = exp(-20.0 * abs(shellPhaseAlt - 0.5));
    uv += radialDir * flowControl * (shell * (0.018 + 0.028 * pulseControl) + shellAlt * 0.012);
    uv *= 1.0 + flowControl * shell * (0.04 + 0.03 * pulseControl);

    radius = length(uv) + 1e-6;
    float baseAngle = atan(uv.y, uv.x);
    float angle = baseAngle;
    depth = -log(radius + 0.06);

    float segments = max(3.0, float(iSegments));
    float segmentJitter = 0.15 * pulseControl * sin(time * 0.6 + radius * 4.0);
    float sector = TAU / segments;
    angle = mod(angle + sector * 0.5 + segmentJitter, sector) - sector * 0.5;
    angle = abs(angle);

    float nestedSegments = max(3.0, segments + 2.0 + floor(pulseControl * 2.0 + warp * 2.0));
    float nestedSector = TAU / nestedSegments;
    float nestedAngle = mod(baseAngle + nestedSector * 0.5 - segmentJitter * 0.7, nestedSector) - nestedSector * 0.5;
    nestedAngle = abs(nestedAngle);

    float twistBase = clamp(iTwist / 100.0, 0.0, 1.0);
    float twist = twistBase * (0.6 + 0.4 * sin(time * 0.5));
    float twistPhase = time * twistBase * 0.4;
    float twistedAngle = angle + radius * twist * 2.0 + twistPhase;
    float nestedTwist = nestedAngle + radius * (0.8 + twist * 1.3) + time * (0.15 + pulseControl * 0.25);

    float waveAngular = sin(twistedAngle * (7.0 + segments * 0.9) - time * (1.6 + pulseControl * 1.2));
    float waveRadial = sin(radius * (18.0 + segments * 1.2 + warp * 18.0) - time * (1.4 + pulseControl * 1.6));
    float waveNested = cos(nestedTwist * (5.0 + nestedSegments * 0.6) + radius * (10.0 + warp * 16.0) - time * (1.1 + warp * 1.8));
    float waveFlow = sin(depth * (5.0 + flowControl * 6.0 + warp * 8.0) + time * (flowRate * (1.1 + flowControl * 0.6)));
    float lattice = sin((uv.x + uv.y) * (7.0 + warp * 12.0) + time * 1.25) *
        cos((uv.x - uv.y) * (6.5 + warp * 11.0) - time * 1.05);
    float blossom = sin((waveAngular + waveNested) * 2.4 + radius * (7.0 + segments) - time * (2.0 + pulseControl * 1.4));
    float spokes = 1.0 - smoothstep(0.08, 0.7, abs(sin(twistedAngle * segments * 1.7 + waveRadial * 0.8)));
    float portal = exp(-radius * (2.8 - pulseControl * 1.2)) * (0.65 + 0.35 * sin(time * (2.4 + pulseControl) + radius * 10.0));
    float pattern = 0.5 + 0.5 * (0.21 * waveAngular + 0.18 * waveRadial + 0.17 * waveNested + 0.16 * lattice + 0.14 * blossom + 0.14 * waveFlow);
    pattern = pow(saturate(pattern), mix(1.5, 0.78, pulseControl));
    pattern *= 0.78 + 0.42 * spokes;
    pattern += portal * (0.12 + 0.18 * pulseControl);
    pattern += flowControl * (shell * (0.08 + 0.18 * pulseControl) + shellAlt * 0.06);

    float hue = fract(
        twistedAngle / max(sector, 1e-4) * 0.11 +
        nestedAngle / max(nestedSector, 1e-4) * 0.09 +
        radius * (0.22 + warp * 0.28) +
        depth * (0.03 + flowControl * 0.04) +
        blossom * 0.08 +
        time * (0.05 + pulseControl * 0.07)
    );
    hue += BASE_COLOR_SHIFT * 0.08 * sin(time * (0.9 + pulseControl * 0.9) + radius * 4.5 + waveAngular * 1.8);
    hue += 0.05 * sin(lattice * 3.0 + waveNested * 2.0 + waveFlow);

    float pulse = 1.0 + 0.75 * pulseControl * sin(time * 2.8 + radius * 7.0 + waveNested * 1.2);
    float shimmer = 0.88 + 0.12 * sin(nestedTwist * 4.0 - time * (2.0 + pulseControl * 1.1) + blossom);
    float depthGlow = 0.72 + 0.20 * sin(radius * 14.0 - time * 2.2 + waveAngular * 1.7) + shell * 0.16 + shellAlt * 0.08;
    vec3 base = palette(hue, iColorMode);
    vec3 accent = base;
    vec3 ghost = base;
    if (iColorMode != 3) {
        accent = palette(hue + 0.17 + blossom * 0.05, (iColorMode + 3) % 9);
        ghost = palette(hue - 0.13 + lattice * 0.03, (iColorMode + 6) % 9);
    }
    vec3 chroma = mix(base, accent, 0.24 + 0.22 * warp);
    chroma = mix(chroma, ghost, 0.10 + 0.18 * pulseControl * saturate(blossom * 0.5 + 0.5));

    float colorEnergy = 1.12 + pulseControl * 0.30 + speedControl * 0.18 + warp * 0.10;
    vec3 color = chroma * pattern * colorEnergy * pulse * shimmer * depthGlow;
    color += accent * portal * (0.08 + 0.11 * flowControl) * (0.48 + pulseControl);
    color += mix(accent, ghost, 0.5) * flowControl * shell * (0.07 + 0.14 * pulseControl);

    float vignette = smoothstep(1.3, 0.2, radius);
    color *= vignette;
    color += ghost * exp(-radius * (3.5 + 1.5 * warp)) * 0.11;
    color = liftMids(color, 0.16 + 0.08 * pulseControl);

    // Hue-preserving soft clamp — prevent white-out at high intensity/pulse
    float peak = max(color.r, max(color.g, color.b));
    if (peak > 1.0) color /= peak;
    color = limitWhiteness(color, 0.34);

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
    } else if (iStyle == 4) {
        // Fractal — recursive petal overlays and a secondary psychedelic bloom
        float recursive = sin(radius * 30.0 - time * 3.0 + blossom * 2.5) *
            cos(twistedAngle * 10.0 + time * 1.8 + waveNested);
        float mandala = smoothstep(0.2, 0.95, 0.5 + 0.5 * recursive);
        vec3 fractalA = base;
        vec3 fractalB = base;
        if (iColorMode != 3) {
            fractalA = palette(hue + recursive * 0.06 + radius * 0.08, (iColorMode + 2) % 9);
            fractalB = palette(hue + recursive * 0.10 + 0.2, (iColorMode + 5) % 9);
        }
        vec3 fractalColor = mix(fractalA, fractalB, 0.5 + 0.5 * sin(time * 0.9));
        color = mix(color, fractalColor * (0.7 + 0.4 * pattern), 0.18 + 0.22 * mandala);
        color += fractalColor * spokes * 0.12;
    }

    float aberration = BASE_ABERRATION * 0.003;
    vec3 rShift = color;
    vec3 gShift = color;
    vec3 bShift = color;
    rShift.r = clamp(color.r + aberration * 0.8, 0.0, 1.0);
    bShift.b = clamp(color.b + aberration * 0.8, 0.0, 1.0);
    color = vec3(rShift.r, gShift.g, bShift.b);
    color = liftMids(color, 0.10);
    peak = max(color.r, max(color.g, color.b));
    if (peak > 1.0) color /= peak;
    color = limitWhiteness(color, 0.32);

    fragColor = vec4(color, 1.0);
}
