#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform vec3 iBackgroundColor;
uniform vec3 iColor1;
uniform vec3 iColor2;
uniform vec3 iColor3;
uniform int iTheme;
uniform float iSpeed;
uniform float iBloom;
uniform float iSpread;
uniform float iDensity;

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

vec3 plasmaPalette(float t, vec3 c1, vec3 c2, vec3 c3) {
    vec3 color = mix(c1, c2, smoothstep(0.04, 0.52, t));
    color = mix(color, c3, smoothstep(0.48, 0.96, t));
    return color;
}

vec3 themedPalette(float t) {
    if (iTheme == 1) return plasmaPalette(t, vec3(0.56, 1.00, 0.28), vec3(0.10, 0.86, 0.86), vec3(0.42, 0.20, 0.98));
    if (iTheme == 2) return plasmaPalette(t, vec3(0.10, 0.88, 0.84), vec3(0.98, 0.22, 0.76), vec3(0.40, 0.18, 0.96));
    if (iTheme == 3) return plasmaPalette(t, vec3(0.94, 0.22, 0.08), vec3(1.00, 0.56, 0.10), vec3(1.00, 0.86, 0.24));
    if (iTheme == 4) return plasmaPalette(t, vec3(0.14, 0.94, 0.54), vec3(0.20, 0.78, 1.00), vec3(0.56, 0.26, 0.98));
    if (iTheme == 5) return plasmaPalette(t, vec3(1.00, 0.28, 0.70), vec3(0.18, 0.74, 1.00), vec3(1.00, 0.84, 0.18));
    if (iTheme == 6) return plasmaPalette(t, vec3(0.86, 1.00, 0.24), vec3(0.10, 0.92, 0.84), vec3(1.00, 0.42, 0.34));
    if (iTheme == 7) return plasmaPalette(t, vec3(0.14, 0.90, 0.92), vec3(0.16, 0.48, 1.00), vec3(0.06, 0.14, 0.54));
    return plasmaPalette(t, iColor1, iColor2, iColor3);
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    float speed = max(iSpeed, 0.2);
    float bloom = clamp(iBloom * 0.01, 0.0, 1.0);
    float spread = clamp(iSpread * 0.01, 0.0, 1.0);
    float density = clamp(iDensity * 0.01, 0.10, 1.0);
    float time = iTime * (0.34 + speed * 0.52);

    vec2 p = uv * 2.0 - 1.0;
    p.x *= iResolution.x / iResolution.y;

    vec2 q = p * mix(2.8, 8.8, density);
    q += vec2(
        sin(p.y * 3.2 + time * 0.88),
        cos(p.x * 2.7 - time * 0.76)
    ) * (0.16 + spread * 0.40);

    float plasma = 0.0;
    plasma += sin(q.x * 1.2 + time * 1.34);
    plasma += sin(q.y * 1.7 - time * 1.08);
    plasma += sin((q.x + q.y) * 1.1 + time * 0.74);
    plasma += sin(length(q - vec2(
        sin(time * 0.46) * 1.5,
        cos(time * 0.34) * 1.3
    )) * 3.1 - time * 1.42);
    plasma += sin(length(q + vec2(
        cos(time * 0.29) * 1.2,
        sin(time * 0.41) * 1.4
    )) * 2.4 + time * 1.18);
    plasma = plasma / 5.0;
    plasma = 0.5 + 0.5 * plasma;

    float bandWave = 0.5 + 0.5 * sin(plasma * mix(9.0, 20.0, density) * 6.28318 - time * 0.8);
    float contour = smoothstep(0.76 - bloom * 0.16, 0.98, bandWave);
    float lava = smoothstep(0.16, 0.94, plasma);
    float zebra = 0.5 + 0.5 * sin((q.x + q.y * 0.8) * mix(1.8, 4.4, density) + time * 0.54);

    float noise = vnoise(uv * vec2(22.0, 16.0) + vec2(time * 0.35, -time * 0.26));
    vec3 palette = themedPalette(plasma);
    vec3 cycle = themedPalette(fract(plasma + time * 0.08 + noise * 0.08 + zebra * 0.05));
    palette = mix(palette, cycle, 0.18 + spread * 0.16);

    float glow = smoothstep(0.30, 1.0, lava + contour * 0.6) * (0.05 + bloom * 0.20);
    vec3 color = iBackgroundColor;
    color += themedPalette(fract(0.12 + zebra * 0.16)) * (0.04 + spread * 0.05) * (0.4 + noise * 0.6);
    color += palette * lava * (0.40 + density * 0.44);
    color += themedPalette(fract(plasma + 0.42)) * contour * (0.12 + bloom * 0.34);
    color += cycle * glow;
    color += mix(palette, cycle, 0.5) * contour * (0.05 + bloom * 0.12);

    float vignette = smoothstep(1.48, 0.10, length(p));
    color *= 0.42 + 0.74 * vignette;

    color = max(color, vec3(0.0));
    color = 1.0 - exp(-color * mix(1.05, 1.78, bloom));
    color = pow(clamp(color, 0.0, 1.0), vec3(0.94));

    fragColor = vec4(color, 1.0);
}
