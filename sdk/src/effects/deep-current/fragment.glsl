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
// Declared so the palette control binds to a uniform; the TS frame hook
// overrides iLeftColor/iRightColor/iBgColor directly from the palette table.
uniform int iPalette;

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

    // ── Continuous wave → ripple mapping (no hard cutoff) ──
    // Why: the previous smoothstep(-0.15, 0.55, ...) + squaring crushed half
    // the wave range to zero. Mapping [-1,1] directly with a gentle power
    // curve keeps crest emphasis while preserving the midtones we want to see.
    float rippleA = pow(wavesA * 0.5 + 0.5, 1.4);
    float rippleB = pow(wavesB * 0.5 + 0.5, 1.4);

    // ── Spatial boundary — which current dominates where ──
    float flowPos = dot(uv, flowDir);
    float boundaryWarp = (fbm(p * 2.0 + time * 0.15) - 0.5) * 0.25;
    float boundary = flowPos + boundaryWarp;

    float blendWidth = 0.03 + blend * 0.25;
    float currentMix = smoothstep(0.5 - blendWidth, 0.5 + blendWidth, boundary);

    // ── Both currents contribute everywhere; dominance just reweights ──
    // Off-side floor raised from 0.08 → 0.35 so non-dominant current stays
    // visible instead of vanishing into the background.
    float leftStrength = mix(1.0, 0.35, currentMix);
    float rightStrength = mix(0.35, 1.0, currentMix);

    float leftContrib = rippleA * leftStrength;
    float rightContrib = rippleB * rightStrength;

    // Hue blend by contribution ratio — louder wave pulls the color
    float hueBlend = rightContrib / (leftContrib + rightContrib + 1e-4);
    vec3 waveColor = mix(iLeftColor, iRightColor, hueBlend);

    // Brightness modulates a saturated base — amplitude, not presence
    float fieldBrightness = max(leftContrib, rightContrib);
    float brightness = 0.3 + 0.7 * fieldBrightness;

    vec3 color = waveColor * brightness;

    // ── Valley accent — background shows only in deep double-troughs ──
    // Where both ripples dip, bgColor reads as shadow/void between currents
    float valleyDepth = 1.0 - max(rippleA, rippleB);
    float valleyMask = smoothstep(0.55, 0.92, valleyDepth);
    color = mix(color, iBgColor, valleyMask * 0.7);

    // ── Collision energy — constructive interference, hottest at the seam ──
    // Boundary-weighted: interference everywhere, but brightest where the
    // two currents actually meet (currentMix ≈ 0.5). Lifts toward white for
    // a real "colliding plasma" pop.
    float constructive = rippleA * rippleB;
    constructive = smoothstep(0.25, 0.85, constructive);
    float boundaryEmphasis = 1.0 - abs(currentMix - 0.5) * 2.0;
    boundaryEmphasis *= boundaryEmphasis;
    vec3 hotspot = mix(waveColor, vec3(1.0), 0.35);
    vec3 boost = hotspot * constructive * (0.35 + boundaryEmphasis * 0.45);
    color = color + boost * (1.0 - color);  // screen blend

    // ── Gentle breathing along flow axis ──
    float shift = 0.97 + 0.03 * sin(dot(p, flowDir) * 2.0 - time * 1.2);
    color *= shift;

    // ── Subtle vignette ──
    float vignette = smoothstep(1.6, 0.3, length(p));
    color *= 0.94 + 0.06 * vignette;

    fragColor = vec4(clamp(color, 0.0, 1.0), 1.0);
}
