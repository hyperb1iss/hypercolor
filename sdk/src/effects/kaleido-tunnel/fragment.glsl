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
const float BASE_SATURATION = 1.2;
const float BASE_COLOR_SHIFT = 1.0;
const float BASE_ABERRATION = 0.2;
const float BASE_MULTI_HUE = 0.6;
const float BASE_PALETTE_DRIFT = 0.4;
const float BASE_SPECTRUM_SPREAD = 1.2;

vec3 hsv2rgb(vec3 c) {
    vec4 k = vec4(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    vec3 p = abs(fract(c.xxx + k.xyz) * 6.0 - k.www);
    return c.z * mix(k.xxx, clamp(p - k.xxx, 0.0, 1.0), c.y);
}

vec3 palette(float t, int mode, float sat) {
    t = fract(t);
    float s = clamp(sat, 0.0, 1.0);
    float drift = BASE_PALETTE_DRIFT * 0.25;
    float spread = BASE_SPECTRUM_SPREAD * 0.15;
    float multiHue = BASE_MULTI_HUE;
    float t2 = fract(t + spread + sin(iTime * 0.17) * drift);
    float t3 = fract(t - spread + cos(iTime * 0.11) * drift);
    vec3 base;

    if (mode == 1) {
        float sector = floor(t * 3.0);
        vec3 triad = sector < 1.0
            ? vec3(1.0, 0.1, 0.8)
            : sector < 2.0
                ? vec3(0.1, 1.0, 0.9)
                : vec3(1.0, 1.0, 0.1);
        float shift = sin(t * TAU * 3.0 + iTime * 0.6) * 0.08;
        vec3 hsv = vec3(fract(t + shift), s, 1.0);
        return mix(triad, hsv2rgb(hsv), 0.5);
    }

    if (mode == 2) {
        return vec3(t * 0.8 + 0.2);
    }

    if (mode == 3) {
        float flash = pow(sin(iTime * 10.0) * 0.5 + 0.5, 4.0);
        base = vec3(0.2, 0.7, 1.6) + flash * 0.2;
    } else if (mode == 4) {
        base = vec3(0.9, 0.3 + 0.1 * sin(iTime * 0.5), 1.2);
    } else if (mode == 5) {
        base = mix(vec3(1.4, 0.6, 0.2), vec3(0.2, 0.2, 0.8), t);
    } else if (mode == 6) {
        base = vec3(0.3, 1.6, 0.4);
    } else if (mode == 7) {
        base = vec3(0.95, 0.4, 0.9);
    } else if (mode == 8) {
        base = vec3(0.05, 0.3, 0.6);
    } else {
        vec3 primary = hsv2rgb(vec3(t, s, 1.0));
        vec3 secondary = hsv2rgb(vec3(t2, s, 1.0));
        vec3 tertiary = hsv2rgb(vec3(t3, s, 1.0));
        return mix(mix(primary, secondary, multiHue * 0.5), tertiary, multiHue * 0.35);
    }

    vec3 c2 = hsv2rgb(vec3(t2, s, 1.0));
    vec3 c3 = hsv2rgb(vec3(t3, s, 1.0));
    return mix(mix(base, c2, multiHue * 0.6), c3, multiHue * 0.4);
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

    float twist = clamp(iTwist / 100.0, 0.0, 1.0) * (0.6 + 0.4 * sin(time * 0.5));
    float twistedAngle = angle + radius * twist * 2.0 + time * twist * 0.4;

    float waveAngular = sin(twistedAngle * (6.0 + segments * 0.5) - time * 1.6);
    float waveRadial = sin(radius * (18.0 + segments * 0.8) - time * 1.2);
    float pattern = 0.5 + 0.5 * (0.6 * waveAngular + 0.4 * waveRadial);

    float pulseControl = clamp(iPulse / 100.0, 0.0, 1.0);
    float hue = fract(twistedAngle / sector + radius * 0.25 + time * 0.06);
    hue += BASE_COLOR_SHIFT * 0.1 * sin(time * (0.6 + 0.4 * pulseControl) + radius * 4.0);

    float pulse = 1.0 + 0.65 * pulseControl * sin(time * 2.5 + radius * 6.0);
    float shimmer = 0.92 + 0.08 * sin(twistedAngle * 3.0 - time * (1.8 + pulseControl));
    float saturation = BASE_SATURATION;
    vec3 base = palette(hue, iColorMode, clamp(saturation * 0.75, 0.0, 1.0));

    float intensity = clamp(iColorIntensity / 100.0, 0.2, 2.2);
    vec3 color = base * pattern * intensity * pulse * shimmer;

    float vignette = smoothstep(1.3, 0.2, radius);
    color *= vignette;

    if (iStyle == 0) {
        float line = step(0.98, fract(gl_FragCoord.y * 0.03 + sin(time) * 0.1));
        color.rb += line * 0.2;
    } else if (iStyle == 2) {
        float scan = 0.7 + 0.3 * sin(gl_FragCoord.y * 0.6 + time * 10.0);
        color *= vec3(0.8, 1.0, 1.1) * scan;
    } else if (iStyle == 1) {
        float grain = fract(sin(dot(gl_FragCoord.xy, vec2(12.9898, 78.233))) * 43758.5453);
        color += (grain - 0.5) * 0.06;
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
