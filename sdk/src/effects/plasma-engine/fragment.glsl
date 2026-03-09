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

vec3 plasmaPalette(float t, vec3 c1, vec3 c2, vec3 c3) {
    vec3 color = mix(c1, c2, smoothstep(0.04, 0.52, t));
    color = mix(color, c3, smoothstep(0.48, 0.96, t));
    return color;
}

vec3 themedPalette(float t) {
    if (iTheme == 1) return plasmaPalette(t, vec3(0.14, 0.92, 0.64), vec3(0.10, 0.86, 0.86), vec3(0.42, 0.20, 0.98));
    if (iTheme == 2) return plasmaPalette(t, vec3(0.10, 0.88, 0.84), vec3(0.98, 0.22, 0.76), vec3(0.40, 0.18, 0.96));
    if (iTheme == 3) return plasmaPalette(t, vec3(0.94, 0.22, 0.08), vec3(1.00, 0.52, 0.08), vec3(0.92, 0.32, 0.62));
    if (iTheme == 4) return plasmaPalette(t, vec3(0.14, 0.94, 0.54), vec3(0.20, 0.78, 1.00), vec3(0.56, 0.26, 0.98));
    if (iTheme == 5) return plasmaPalette(t, vec3(1.00, 0.28, 0.70), vec3(0.18, 0.74, 1.00), vec3(1.00, 0.56, 0.12));
    if (iTheme == 6) return plasmaPalette(t, vec3(0.18, 0.94, 0.74), vec3(0.10, 0.92, 0.84), vec3(1.00, 0.40, 0.32));
    if (iTheme == 7) return plasmaPalette(t, vec3(0.14, 0.90, 0.92), vec3(0.16, 0.48, 1.00), vec3(0.06, 0.14, 0.54));
    return plasmaPalette(t, iColor1, iColor2, iColor3);
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    float speed = max(iSpeed, 0.2);
    float glow = clamp(iBloom * 0.01, 0.0, 1.0);
    float spread = clamp(iSpread * 0.01, 0.0, 1.0);
    float density = clamp(iDensity * 0.01, 0.10, 1.0);
    float time = iTime * (0.18 + speed * 0.22);

    vec2 p = uv * 2.0 - 1.0;
    p.x *= iResolution.x / iResolution.y;

    vec2 q = p * mix(1.6, 4.2, density);
    vec2 drift = vec2(
        sin(p.y * 1.9 + time * 0.61) + cos(p.y * 0.7 - time * 0.29),
        cos(p.x * 1.7 - time * 0.57) + sin(p.x * 0.9 + time * 0.23)
    );
    q += drift * (0.08 + spread * 0.22);

    float plasma = 0.0;
    plasma += sin(q.x + time * 0.81);
    plasma += sin(q.y - time * 0.63);
    plasma += sin((q.x + q.y) * 0.75 + time * 0.41);
    plasma += sin(length(q - vec2(
        cos(time * 0.11) * 2.0,
        sin(time * 0.19) * 1.5
    )) * 1.4 + time * 0.37);
    plasma += sin(length(q + vec2(
        sin(time * 0.17) * 1.8,
        cos(time * 0.13) * 1.6
    )) * 1.9 - time * 0.49);
    plasma = plasma / 5.0;
    plasma = 0.5 + 0.5 * plasma;

    float paletteShift = time * (0.015 + speed * 0.010);
    vec3 palette = themedPalette(fract(plasma + paletteShift));
    float body = smoothstep(0.08, 0.92, plasma);
    float highlight = pow(body, mix(2.4, 1.4, glow));

    vec3 color = mix(iBackgroundColor * 0.92, palette, 0.24 + body * 0.76);
    color *= 0.62 + body * 0.48;
    color += palette * highlight * (0.03 + glow * 0.09);

    float vignette = smoothstep(1.42, 0.18, length(p));
    color *= 0.84 + 0.16 * vignette;

    color = pow(clamp(color, 0.0, 1.0), vec3(0.94));

    fragColor = vec4(color, 1.0);
}
