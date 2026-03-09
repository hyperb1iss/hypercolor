#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;
uniform int iPalette;
uniform float iSpeed;
uniform float iFlow;
uniform float iTurbulence;
uniform float iSaturation;

// --- Hash & noise primitives ---

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

mat2 rot(float a) {
    float s = sin(a);
    float c = cos(a);
    return mat2(c, -s, s, c);
}

// --- FBM with per-octave rotation (critical for fluid swirl quality) ---

float fbm(vec2 p) {
    float sum = 0.0;
    float amp = 0.5;
    float freq = 1.0;
    for (int i = 0; i < 4; i++) {
        sum += amp * vnoise(p * freq);
        p = rot(0.5) * p * 2.0 + vec2(1.7, 9.2);
        amp *= 0.5;
        freq *= 2.0;
    }
    return sum;
}

// --- Palette system (4 colors: deep, mid, bright, accent) ---

struct InkPalette {
    vec3 deep;
    vec3 mid;
    vec3 bright;
    vec3 accent;
};

InkPalette getPalette(int id) {
    InkPalette pal;
    if (id == 1) {
        // Sakura
        pal.deep   = vec3(0.039, 0.000, 0.031);
        pal.mid    = vec3(0.400, 0.000, 0.200);
        pal.bright = vec3(1.000, 0.267, 0.533);
        pal.accent = vec3(1.000, 0.667, 0.800);
    } else if (id == 2) {
        // Poison
        pal.deep   = vec3(0.000, 0.031, 0.016);
        pal.mid    = vec3(0.000, 0.200, 0.000);
        pal.bright = vec3(0.000, 1.000, 0.267);
        pal.accent = vec3(0.533, 1.000, 0.000);
    } else if (id == 3) {
        // Molten
        pal.deep   = vec3(0.031, 0.008, 0.000);
        pal.mid    = vec3(0.267, 0.000, 0.000);
        pal.bright = vec3(1.000, 0.267, 0.000);
        pal.accent = vec3(1.000, 0.667, 0.000);
    } else if (id == 4) {
        // Arctic
        pal.deep   = vec3(0.000, 0.016, 0.063);
        pal.mid    = vec3(0.000, 0.102, 0.267);
        pal.bright = vec3(0.267, 0.667, 1.000);
        pal.accent = vec3(0.667, 0.867, 1.000);
    } else if (id == 5) {
        // Phantom
        pal.deep   = vec3(0.016, 0.000, 0.031);
        pal.mid    = vec3(0.133, 0.000, 0.267);
        pal.bright = vec3(0.533, 0.000, 1.000);
        pal.accent = vec3(0.733, 0.533, 1.000);
    } else {
        // Abyss (default)
        pal.deep   = vec3(0.004, 0.004, 0.031);
        pal.mid    = vec3(0.000, 0.067, 0.267);
        pal.bright = vec3(0.000, 0.667, 0.800);
        pal.accent = vec3(0.133, 0.400, 1.000);
    }
    return pal;
}

// --- Approximate Oklab mixing for perceptually smooth transitions ---

vec3 srgbToLinear(vec3 c) {
    vec3 s = max(c, 0.0);
    return mix(s / 12.92, pow((s + 0.055) / 1.055, vec3(2.4)), step(vec3(0.04045), s));
}

vec3 linearToSrgb(vec3 c) {
    vec3 s = max(c, 0.0);
    return mix(s * 12.92, 1.055 * pow(s, vec3(1.0 / 2.4)) - 0.055, step(vec3(0.0031308), s));
}

vec3 linearToOklab(vec3 c) {
    float l = 0.4122214708 * c.r + 0.5363325363 * c.g + 0.0514459929 * c.b;
    float m = 0.2119034982 * c.r + 0.6806995451 * c.g + 0.1073969566 * c.b;
    float s = 0.0883024619 * c.r + 0.2817188376 * c.g + 0.6299787005 * c.b;

    float l_ = pow(max(l, 0.0), 1.0 / 3.0);
    float m_ = pow(max(m, 0.0), 1.0 / 3.0);
    float s_ = pow(max(s, 0.0), 1.0 / 3.0);

    return vec3(
        0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
        1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
        0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_
    );
}

vec3 oklabToLinear(vec3 lab) {
    float l_ = lab.x + 0.3963377774 * lab.y + 0.2158037573 * lab.z;
    float m_ = lab.x - 0.1055613458 * lab.y - 0.0638541728 * lab.z;
    float s_ = lab.x - 0.0894841775 * lab.y - 1.2914855480 * lab.z;

    float l = l_ * l_ * l_;
    float m = m_ * m_ * m_;
    float s = s_ * s_ * s_;

    return vec3(
         4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s
    );
}

vec3 mixOklab(vec3 a, vec3 b, float t) {
    vec3 labA = linearToOklab(srgbToLinear(a));
    vec3 labB = linearToOklab(srgbToLinear(b));
    vec3 blended = mix(labA, labB, clamp(t, 0.0, 1.0));
    return linearToSrgb(oklabToLinear(blended));
}

// --- Main ---

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    vec2 p = uv * 2.0 - 1.0;
    p.x *= iResolution.x / iResolution.y;

    // Normalize controls
    float speed = max(iSpeed, 0.2);
    float flow = clamp(iFlow * 0.01, 0.0, 1.0);
    float turb = clamp(iTurbulence * 0.01, 0.0, 1.0);
    float sat = clamp(iSaturation * 0.01, 0.0, 1.0);

    float time = iTime * (0.04 + speed * 0.025);

    // Spatial scale — low frequency for large LED-spanning structures
    float scale = mix(1.5, 2.5, turb);
    vec2 st = p * scale;

    // Warp strength — how aggressively the fluid folds
    float warpAmt = mix(2.0, 5.0, flow);

    // === Double-nested domain warping (iq technique) ===

    // First warp layer: establishes large flow structures
    vec2 q = vec2(
        fbm(st + vec2(0.0, 0.0) + time * 0.80),
        fbm(st + vec2(5.2, 1.3) + time * 1.00)
    );

    // Second warp layer: nested on q — this is the magic
    vec2 r = vec2(
        fbm(st + warpAmt * q + vec2(1.7, 9.2) + time * 1.20),
        fbm(st + warpAmt * q + vec2(8.3, 2.8) + time * 1.00)
    );

    // Final pattern value
    float f = fbm(st + warpAmt * r);

    // === Color mapping using flow structure ===

    InkPalette pal = getPalette(iPalette);

    // Base color from final pattern density
    float density = clamp(f * f * 4.0, 0.0, 1.0);
    vec3 color = mixOklab(pal.deep, pal.mid, density);

    // q-vector adds flow-following color variation
    float qInfluence = clamp(length(q), 0.0, 1.0);
    color = mixOklab(color, pal.bright, qInfluence * 0.7);

    // r-vector adds accent tones at fold boundaries
    float rInfluence = clamp(r.y * 0.5 + 0.25, 0.0, 1.0);
    color = mixOklab(color, pal.accent, rInfluence * 0.4);

    // Cross-flow luminance variation from q direction
    float flowAngle = atan(q.y, q.x + 0.0001);
    float flowShimmer = 0.5 + 0.5 * sin(flowAngle * 3.0 + time * 2.0);
    color = mixOklab(color, pal.bright, flowShimmer * density * 0.2);

    // === Saturation control ===
    float luminance = dot(color, vec3(0.2126, 0.7152, 0.0722));
    color = mix(vec3(luminance), color, 0.4 + sat * 0.9);

    // === Tone mapping (HDR-like) ===
    float exposure = 1.4 + flow * 0.3;
    color = 1.0 - exp(-color * exposure);

    // Subtle vignette for depth
    float vignette = smoothstep(1.5, 0.2, length(p));
    color *= 0.82 + 0.18 * vignette;

    // Final gamma
    color = pow(clamp(color, 0.0, 1.0), vec3(0.95));

    fragColor = vec4(color, 1.0);
}
