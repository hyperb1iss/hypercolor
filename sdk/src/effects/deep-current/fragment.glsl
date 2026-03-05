#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

// Controls
uniform vec3 iLeftColor;
uniform vec3 iRightColor;
uniform float iSpeed;
uniform float iRippleIntensity;
uniform float iParticleAmount;
uniform float iBlend;
uniform int iSplitMode;

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

float splitCoord(vec2 uv, int mode) {
    if (mode == 1) return uv.y;
    if (mode == 2) return dot(uv, vec2(0.70710678));
    return uv.x;
}

float particleLayer(vec2 uv, float time, float scale, float drift, float seed) {
    vec2 p = uv * scale;
    vec2 cell = floor(p);
    vec2 local = fract(p) - 0.5;

    float id = hash21(cell + seed);
    float twinkle = 0.55 + 0.45 * sin(time * (4.0 + id * 3.5) + id * 18.0);
    float rise = fract(id * 5.7 + time * drift) - 0.5;
    float sway = sin(time * (1.8 + id * 2.4) + id * 12.0) * 0.28;
    vec2 offset = vec2(sway, rise) * 0.38;

    float d = length(local - offset);
    float sparkle = smoothstep(0.18, 0.0, d);
    return sparkle * twinkle;
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    float time = iTime * (0.35 + iSpeed * 0.9);
    float ripple = clamp(iRippleIntensity * 0.01, 0.0, 1.0);
    float particleAmount = clamp(iParticleAmount * 0.01, 0.0, 1.0);
    float blend = clamp(iBlend * 0.01, 0.0, 1.0);

    float seamBase = splitCoord(uv, iSplitMode);
    float seamCenter = 0.5 + 0.015 * sin(time * 0.23) + 0.01 * sin(time * 0.51 + 1.7);

    vec2 centerA = vec2(0.22 + 0.12 * sin(time * 0.29), 0.42 + 0.11 * cos(time * 0.25));
    vec2 centerB = vec2(0.78 + 0.1 * cos(time * 0.27 + 0.8), 0.58 + 0.1 * sin(time * 0.34));
    float distA = length(uv - centerA);
    float distB = length(uv - centerB);

    float ringFreqA = mix(22.0, 44.0, ripple);
    float ringFreqB = mix(18.0, 38.0, ripple);
    float rippleWaveA = sin(distA * ringFreqA - time * (2.8 + ripple * 4.5));
    float rippleWaveB = sin(distB * ringFreqB - time * (2.2 + ripple * 3.6) + 1.4);
    float rippleWave = 0.55 * rippleWaveA + 0.45 * rippleWaveB;

    float seamShift = rippleWave * (0.002 + 0.018 * ripple);
    float seamWidth = mix(0.002, 0.17, blend);
    float seamMix = smoothstep(seamCenter - seamWidth, seamCenter + seamWidth, seamBase + seamShift);

    vec3 col = mix(iLeftColor, iRightColor, seamMix);

    float rippleLinesA = smoothstep(0.88, 0.995, abs(sin(distA * ringFreqA - time * (3.2 + ripple * 5.8))));
    float rippleLinesB = smoothstep(0.9, 0.995, abs(sin(distB * ringFreqB - time * (2.6 + ripple * 4.9))));
    float rippleLines = (rippleLinesA + rippleLinesB) * 0.5 * ripple;
    vec3 rippleTint = mix(iLeftColor, iRightColor, 0.5 + 0.5 * rippleWave);
    col += rippleTint * rippleLines * 0.45;

    float dividerWidth = 0.003 + (1.0 - blend) * 0.01;
    float divider = 1.0 - smoothstep(dividerWidth, dividerWidth + 0.012, abs((seamBase + seamShift) - seamCenter));
    col += mix(iLeftColor, iRightColor, 0.5) * divider * (0.2 + ripple * 0.35);
    col += vec3(1.0, 0.96, 0.87) * divider * 0.35;

    float p1 = particleLayer(uv + vec2(0.0, time * 0.03), time, mix(14.0, 36.0, particleAmount), 0.28 + particleAmount * 0.95, 1.37);
    float p2 = particleLayer(uv + vec2(-time * 0.018, 0.0), time * 1.18, mix(22.0, 52.0, particleAmount), 0.4 + particleAmount * 1.1, 7.91);
    float particles = (p1 * 0.8 + p2 * 0.6) * mix(0.2, 1.0, particleAmount);
    vec3 particleColor = mix(iLeftColor, iRightColor, seamMix);
    col += particleColor * particles * (0.4 + particleAmount * 0.65);
    col += vec3(1.0, 0.95, 0.88) * particles * 0.25;

    float verticalShade = 0.88 + 0.18 * smoothstep(0.0, 1.0, uv.y);
    float vignette = 1.0 - 0.28 * dot((uv - 0.5) * vec2(1.35, 1.0), (uv - 0.5) * vec2(1.35, 1.0));
    col *= verticalShade * vignette;

    col = col / (1.0 + col * 0.55);
    col = pow(clamp(col, 0.0, 1.0), vec3(0.92));

    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
