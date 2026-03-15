#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform vec3 iLeftColor;
uniform vec3 iRightColor;
uniform vec3 iBgColor;
uniform float iSpeed;
uniform float iTurbulence;
uniform float iFlow;
uniform float iBlend;
uniform int iDirection;

// ─── Noise ──────────────────────────────────────────────────────────

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

float vnoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    // Quintic interpolation — sharper ridges than hermite
    f = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
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
        p = p * 2.05 + vec2(1.7, 9.2);
        amp *= 0.48;
    }
    return sum;
}

vec2 rotVec(vec2 v, float a) {
    float s = sin(a), c = cos(a);
    return vec2(v.x * c - v.y * s, v.x * s + v.y * c);
}

// ─── Wave field — sum of directional traveling waves ────────────────

float waveField(vec2 p, float time, vec2 baseDir, float turb, float flowSpeed) {
    float sum = 0.0;
    float totalAmp = 0.0;

    for (int i = 0; i < 5; i++) {
        float fi = float(i);

        // Each wave has a slightly different angle — spread by turbulence
        float angleSpread = (fi - 2.0) * turb * 0.28;
        // Angles slowly drift over time → interference pattern evolves
        angleSpread += 0.18 * sin(time * (0.2 + fi * 0.07) + fi * 1.7);

        vec2 dir = rotVec(baseDir, angleSpread);

        // Decreasing frequency per wave for broad-to-fine structure
        float freq = 4.5 + fi * 2.2;

        // Phase: position along wave direction * frequency - time * speed
        float phase = dot(p, dir) * freq - time * flowSpeed * (1.0 + fi * 0.15);

        // Noise-warp the phase for organic, non-uniform wave fronts
        phase += vnoise(p * 1.8 + fi * 3.3 + time * 0.08) * turb * 2.5;

        float amp = 1.0 / (1.0 + fi * 0.4);
        sum += sin(phase) * amp;
        totalAmp += amp;
    }

    return sum / totalAmp; // normalized to ~[-1, 1]
}

// ─── Main ───────────────────────────────────────────────────────────

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    float aspect = iResolution.x / iResolution.y;
    vec2 p = (uv - 0.5) * vec2(aspect, 1.0);

    float speed = max(iSpeed, 0.2);
    float turb = clamp(iTurbulence * 0.01, 0.0, 1.0);
    float flow = clamp(iFlow * 0.01, 0.0, 1.0);
    float blend = clamp(iBlend * 0.01, 0.0, 1.0);

    float time = iTime * (0.4 + speed * 0.2);

    // ── Flow axis ──
    vec2 flowDir;
    if (iDirection == 1) flowDir = vec2(0.0, 1.0);
    else if (iDirection == 2) flowDir = normalize(vec2(1.0, 1.0));
    else flowDir = vec2(1.0, 0.0);

    // ── Two opposing wave fields ──
    float flowSpeed = 1.5 + flow * 3.0;

    float wavesA = waveField(p, time, flowDir, turb, flowSpeed);
    float wavesB = waveField(p, time, -flowDir, turb, flowSpeed * 0.85);

    // ── Convert wave amplitude to ripple brightness ──
    // Tight window + power curve = sharp crests, true-black valleys
    float rippleA = smoothstep(-0.15, 0.55, wavesA);
    rippleA = rippleA * rippleA;  // square for LED-punchy falloff
    float rippleB = smoothstep(-0.15, 0.55, wavesB);
    rippleB = rippleB * rippleB;

    // ── Spatial boundary — which current dominates where ──
    float flowPos = dot(uv, flowDir);
    float boundaryWarp = (fbm(p * 2.0 + time * 0.15) - 0.5) * 0.25;
    float boundary = flowPos + boundaryWarp;

    float blendWidth = 0.03 + blend * 0.25;
    float currentMix = smoothstep(0.5 - blendWidth, 0.5 + blendWidth, boundary);

    // ── Color composition — blend, never accumulate ──
    // Wave intensity weighted by spatial dominance
    float leftStrength = mix(1.0, 0.08, currentMix);
    float rightStrength = mix(0.08, 1.0, currentMix);

    float leftIntensity = rippleA * leftStrength;
    float rightIntensity = rippleB * rightStrength;
    float totalIntensity = leftIntensity + rightIntensity;

    // Hue selection — interpolate between the two wave colors by dominance
    // When only one wave fires, it's pure. Both active → hue blend, not sum.
    float hueBlend = (totalIntensity > 0.001)
        ? rightIntensity / totalIntensity
        : currentMix;
    vec3 waveColor = mix(iLeftColor, iRightColor, hueBlend);

    // How much wave vs background at this pixel
    float wavePresence = clamp(totalIntensity, 0.0, 1.0);

    // Compose: background → wave color (never additive, never exceeds input)
    vec3 color = mix(iBgColor, waveColor, wavePresence);

    // ── Constructive interference — screen blend for brightness boost ──
    // Where both crests align, push brightness without exceeding 1.0
    float constructive = rippleA * rippleB;
    vec3 boost = waveColor * constructive * 0.3;
    color = color + boost * (1.0 - color);  // screen blend

    // ── Subtle low-freq brightness variation ──
    float shift = 0.93 + 0.07 * sin(dot(p, flowDir) * 2.0 - time * 1.2);
    color *= shift;

    // ── Gentle vignette ──
    float vignette = smoothstep(1.5, 0.2, length(p));
    color *= 0.9 + 0.1 * vignette;

    fragColor = vec4(clamp(color, 0.0, 1.0), 1.0);
}
