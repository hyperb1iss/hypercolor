#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iAudioBass;
uniform float iAudioMid;
uniform float iAudioTreble;
uniform float iAudioBeatPulse;
uniform float iAudioLevel;

uniform float iSpeed;
uniform float iGridDensity;
uniform float iSunSize;
uniform float iGlow;
uniform int iPalette;

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    // Synthwave: deep purple → hot pink → orange
    if (id == 0) return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
    if (id == 1) return iqPalette(t, vec3(0.3, 0.1, 0.4), vec3(0.5, 0.3, 0.5), vec3(0.8, 0.5, 0.7), vec3(0.9, 0.2, 0.4));
    if (id == 2) return iqPalette(t, vec3(0.5, 0.2, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 1.0), vec3(0.8, 0.1, 0.6));
    if (id == 3) return iqPalette(t, vec3(0.5, 0.2, 0.0), vec3(0.5, 0.4, 0.2), vec3(1.0, 0.7, 0.4), vec3(0.0, 0.15, 0.2));
    if (id == 4) return iqPalette(t, vec3(0.2, 0.5, 0.4), vec3(0.3, 0.4, 0.4), vec3(0.8, 0.7, 0.9), vec3(0.6, 0.3, 0.7));
    return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    float aspect = iResolution.x / iResolution.y;

    float time = iTime * iSpeed * 0.3;
    float bass = iAudioBass;
    float beatPulse = iAudioBeatPulse;

    vec3 col = vec3(0.0);

    // Sky gradient — dark purple to warm horizon
    float skyGrad = uv.y;
    vec3 skyTop = paletteColor(0.7, iPalette) * 0.1;
    vec3 skyBottom = paletteColor(0.0, iPalette) * 0.4;
    vec3 sky = mix(skyBottom, skyTop, smoothstep(0.4, 0.9, skyGrad));
    col = sky;

    // Retrowave sun
    float sunY = 0.55 + bass * 0.02;
    float sunRadius = iSunSize * 0.003 + beatPulse * 0.02;
    vec2 sunCenter = vec2(0.5, sunY);
    vec2 sunUV = vec2((uv.x - 0.5) * aspect, uv.y) - vec2(0.0, sunY);
    float sunDist = length(sunUV);

    // Sun body with horizontal slice lines
    float sun = smoothstep(sunRadius + 0.005, sunRadius - 0.005, sunDist);
    // Horizontal bands cut through the sun
    float bands = step(0.0, sin((uv.y - sunY) * 80.0 - time * 2.0));
    float bandMask = smoothstep(sunY - sunRadius, sunY - sunRadius * 0.3, uv.y);
    sun *= mix(1.0, bands, bandMask * 0.5);

    vec3 sunColor = mix(
        paletteColor(0.15, iPalette),
        paletteColor(0.0, iPalette),
        smoothstep(sunY - sunRadius, sunY + sunRadius, uv.y)
    );
    col += sunColor * sun * 1.5;

    // Sun glow
    float sunGlow = exp(-sunDist * sunDist * 15.0) * iGlow * 0.008;
    col += paletteColor(0.1, iPalette) * sunGlow;

    // Perspective grid floor
    float horizon = 0.42;
    if (uv.y < horizon) {
        float floorY = horizon - uv.y;
        float depth = 0.1 / (floorY + 0.001);

        // Grid lines — Z direction (horizontal on floor)
        float gridZ = depth * 0.5 + time * 2.0;
        float lineZ = smoothstep(0.05, 0.0, abs(fract(gridZ) - 0.5) - 0.47);
        lineZ *= smoothstep(0.0, 0.05, floorY);

        // Grid lines — X direction (vertical on floor)
        float gridX = (uv.x - 0.5) * depth * iGridDensity * 0.1;
        float lineX = smoothstep(0.05, 0.0, abs(fract(gridX) - 0.5) - 0.47);
        lineX *= smoothstep(0.0, 0.05, floorY);

        float grid = max(lineX, lineZ);

        // Grid pulses on beat
        grid *= 0.6 + beatPulse * 0.4;

        // Grid color — brighter near horizon, fades with distance
        float distFade = exp(-floorY * 8.0);
        vec3 gridColor = paletteColor(0.5 + depth * 0.01, iPalette);
        col += gridColor * grid * distFade * iGlow * 0.012;

        // Floor base darkness
        col *= 0.3 + 0.7 * smoothstep(0.0, horizon, uv.y);

        // Audio-reactive grid height displacement
        float waveX = sin((uv.x - 0.5) * 20.0 + time * 3.0) * bass * 0.02;
        float waveLine = smoothstep(0.003, 0.0, abs(floorY - 0.1 - waveX)) * distFade;
        col += paletteColor(0.3, iPalette) * waveLine * 0.5;
    }

    // Horizon glow line
    float horizonGlow = exp(-abs(uv.y - horizon) * 60.0);
    col += paletteColor(0.2, iPalette) * horizonGlow * iGlow * 0.005 * (1.0 + beatPulse * 0.5);

    // Scanlines overlay
    float scanline = 0.95 + 0.05 * sin(gl_FragCoord.y * 1.5);
    col *= scanline;

    col = col / (1.0 + col * 0.3);
    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
