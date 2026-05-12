#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;
uniform vec3 iBgColor;
uniform vec3 iColor;
uniform vec3 iColor2;
uniform vec3 iColor3;
uniform int iColorMode;
uniform float iCount;
uniform float iSize;
uniform float iSpeed;
uniform int iTheme;

const int MAX_BUBBLES = 120;

struct Palette {
    vec3 primary;
    vec3 secondary;
    vec3 accent;
};

struct BubbleColors {
    vec3 aura;
    vec3 body;
    vec3 gloss;
    vec3 rim;
};

float hash11(float p) {
    p = fract(p * 0.1031);
    p *= p + 33.33;
    p *= p + p;
    return fract(p);
}

float pingPong(float value) {
    float t = mod(value, 2.0);
    return 1.0 - abs(t - 1.0);
}

vec3 saturateColor(vec3 color, float amount, float valueScale) {
    float luma = dot(color, vec3(0.2126, 0.7152, 0.0722));
    return clamp(mix(vec3(luma), color, amount) * valueScale, 0.0, 1.0);
}

vec3 smoothMix(vec3 a, vec3 b, float t) {
    t = clamp(t, 0.0, 1.0);
    t = t * t * (3.0 - 2.0 * t);
    return mix(a, b, t);
}

Palette themePalette(int theme) {
    if (theme == 0) return Palette(vec3(1.0, 0.3098, 0.6039), vec3(1.0, 0.4549, 0.7725), vec3(0.5412, 0.3608, 1.0));
    if (theme == 1) return Palette(vec3(1.0, 0.7020, 0.2784), vec3(1.0, 0.4784, 0.1843), vec3(1.0, 0.3294, 0.4706));
    if (theme == 3) return Palette(vec3(0.0314, 0.9686, 0.9961), vec3(1.0, 0.0235, 0.7098), vec3(0.4353, 0.1765, 1.0));
    if (theme == 4) return Palette(vec3(0.5412, 0.4863, 1.0), vec3(1.0, 0.4980, 0.8118), vec3(0.4627, 1.0, 0.9451));
    if (theme == 5) return Palette(vec3(0.2745, 0.9451, 0.8627), vec3(0.3647, 0.6588, 1.0), vec3(0.0902, 0.2745, 1.0));
    if (theme == 6) return Palette(vec3(0.6235, 0.4471, 1.0), vec3(1.0, 0.3686, 0.7843), vec3(0.4, 0.8314, 1.0));
    if (theme == 7) return Palette(vec3(0.2118, 1.0, 0.6039), vec3(0.0941, 0.8941, 1.0), vec3(1.0, 0.3059, 0.8196));
    return Palette(iColor, iColor2, iColor3);
}

vec3 paletteColor(float index, Palette palette) {
    if (index < 0.5) return palette.primary;
    if (index < 1.5) return palette.secondary;
    return palette.accent;
}

vec3 paletteGradient(float phase, Palette palette) {
    float t = fract(phase);
    if (t < 0.3333333) return smoothMix(palette.primary, palette.secondary, t * 3.0);
    if (t < 0.6666667) return smoothMix(palette.secondary, palette.accent, (t - 0.3333333) * 3.0);
    return smoothMix(palette.accent, palette.primary, (t - 0.6666667) * 3.0);
}

void paletteSet(float band, Palette palette, out vec3 body, out vec3 rim, out vec3 gloss) {
    float wrapped = mod(floor(band), 3.0);
    if (wrapped < 0.5) {
        body = palette.primary;
        rim = palette.secondary;
        gloss = palette.accent;
        return;
    }
    if (wrapped < 1.5) {
        body = palette.secondary;
        rim = palette.accent;
        gloss = palette.primary;
        return;
    }
    body = palette.accent;
    rim = palette.primary;
    gloss = palette.secondary;
}

BubbleColors resolveBubbleColors(float id, float mixSeed, float bandSeed, float blendSeed, int mode, int theme, Palette palette) {
    vec3 body;
    vec3 rim;
    vec3 gloss;

    if (mode == 1) {
        body = paletteGradient(mixSeed + blendSeed * 0.28, palette);
        rim = paletteGradient(mixSeed + 0.26 + blendSeed * 0.14, palette);
        gloss = paletteGradient(mixSeed + 0.54, palette);
    } else if (mode == 2) {
        body = palette.primary;
        rim = palette.secondary;
        gloss = palette.accent;
    } else if (mode == 3) {
        paletteSet(bandSeed, palette, body, rim, gloss);
        if (theme == 3) {
            if (mod(floor(bandSeed), 3.0) < 0.5) {
                body = palette.secondary;
                rim = palette.primary;
                gloss = palette.accent;
            } else if (mod(floor(bandSeed), 3.0) < 1.5) {
                body = palette.primary;
                rim = palette.secondary;
                gloss = palette.accent;
            } else {
                body = palette.accent;
                rim = palette.primary;
                gloss = palette.secondary;
            }
        }
    } else {
        vec3 baseBody;
        vec3 baseRim;
        vec3 baseGloss;
        vec3 nextBody;
        vec3 nextRim;
        vec3 nextGloss;
        paletteSet(bandSeed, palette, baseBody, baseRim, baseGloss);
        paletteSet(bandSeed + 1.0, palette, nextBody, nextRim, nextGloss);
        float blend = smoothstep(0.15, 0.85, blendSeed);
        body = mix(baseBody, nextBody, blend * 0.52);
        rim = mix(baseRim, nextRim, blend * 0.4);
        gloss = mix(baseGloss, nextGloss, blend * 0.34);
    }

    body = saturateColor(body, 1.55, 0.78);
    rim = saturateColor(rim, 1.7, 0.88);
    gloss = saturateColor(gloss, 1.5, 0.82);
    return BubbleColors(saturateColor(mix(body, rim, 0.18), 1.6, 0.74), body, gloss, rim);
}

float circleMask(float dist, float radius, float aa) {
    return 1.0 - smoothstep(radius - aa, radius + aa, dist);
}

float ringMask(float dist, float radius, float width, float aa) {
    return 1.0 - smoothstep(width - aa, width + aa, abs(dist - radius));
}

float waveNoise(float angle, float seed, float time) {
    return
        sin(angle * 3.0 + seed * 6.2831853 + time * 0.5) * 0.45 +
        sin(angle * 5.0 - seed * 4.1 + time * 0.32) * 0.3 +
        sin(angle * 9.0 + seed * 9.7 - time * 0.27) * 0.25;
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    float aspect = iResolution.x / iResolution.y;
    Palette palette = themePalette(iTheme);

    vec3 color = iBgColor * 0.55;
    vec2 washCenter = vec2(0.24 + sin(iTime * 0.18) * 0.06, 0.26 + cos(iTime * 0.15) * 0.05);
    float washDist = length(vec2((uv.x - washCenter.x) * aspect, uv.y - washCenter.y));
    float wash = smoothstep(0.9, 0.0, washDist);
    color += saturateColor(palette.primary, 1.8, 0.62) * wash * 0.018;
    color += saturateColor(palette.secondary, 1.8, 0.62) * wash * 0.012;

    float count = clamp(floor(iCount + 0.5), 10.0, float(MAX_BUBBLES));
    float sizeScale = max(0.2, iSize / 5.0);
    float speedScale = max(iSpeed, 0.0) * 0.006;
    int mode = iColorMode;

    for (int i = 0; i < MAX_BUBBLES; i++) {
        float id = float(i);
        if (id >= count) break;

        float h0 = hash11(id * 11.13 + 1.0);
        float h1 = hash11(id * 17.71 + 2.0);
        float h2 = hash11(id * 23.37 + 3.0);
        float h3 = hash11(id * 31.91 + 4.0);
        float h4 = hash11(id * 43.29 + 5.0);
        float h5 = hash11(id * 59.47 + 6.0);
        float h6 = hash11(id * 71.83 + 7.0);
        float h7 = hash11(id * 83.63 + 8.0);

        float driftBias = 0.84 + h4 * 0.44;
        float pulse = 0.9 + 0.12 * sin(iTime * (1.0 + driftBias * 0.4) + h5 * 6.2831853);
        float radius = mix(0.07, 0.14, h0) * sizeScale * pulse;
        float marginX = min(0.45, radius / max(aspect, 0.001));
        float marginY = min(0.45, radius);

        vec2 velocity = normalize(vec2(h1 * 2.0 - 1.0, h2 * 2.0 - 1.0) + vec2(0.001, -0.002));
        vec2 rawPos = vec2(h3, h4) + velocity * iTime * speedScale * driftBias;
        vec2 pos = vec2(
            mix(marginX, 1.0 - marginX, pingPong(rawPos.x)),
            mix(marginY, 1.0 - marginY, pingPong(rawPos.y))
        );
        vec2 wobble = vec2(sin(iTime * 0.5 + h5 * 6.2831853), cos(iTime * 0.42 + h6 * 6.2831853));
        pos += wobble * vec2(0.018 / max(aspect, 0.001), 0.018) * driftBias;
        pos = clamp(pos, vec2(marginX, marginY), vec2(1.0 - marginX, 1.0 - marginY));

        vec2 delta = vec2((uv.x - pos.x) * aspect, uv.y - pos.y);
        float dist = length(delta);
        BubbleColors bubble = resolveBubbleColors(id, h5, floor(h6 * 3.0), h7, mode, iTheme, palette);

        float angle = atan(delta.y, delta.x);
        float ripple = waveNoise(angle, h5, iTime);
        float edgeRadius = radius * (0.94 + ripple * 0.024);
        float aa = max(fwidth(dist) * 1.05, 0.001);
        float surface = circleMask(dist, edgeRadius, aa);
        float radial = clamp(1.0 - dist / max(edgeRadius, 0.0001), 0.0, 1.0);
        float body = surface * (0.86 + pow(radial, 0.9) * 0.14);
        vec2 highlightDir = normalize(vec2(cos(h6 * 6.2831853), sin(h6 * 6.2831853)) * 0.45 + vec2(-0.62, -0.78));
        float crescentAxis = dot(normalize(delta + vec2(0.0001)), highlightDir);
        float arcBreakup = smoothstep(-0.35, 0.9, waveNoise(angle + h7 * 2.4, h5, iTime * 0.35));
        float rim = smoothstep(0.58, 0.9, 1.0 - radial) * surface;
        float edge = smoothstep(0.88, 0.99, 1.0 - radial) * surface;
        float edgeLight = mix(0.34, 0.74, smoothstep(-0.08, 0.78, crescentAxis)) * (0.76 + arcBreakup * 0.24);
        float coverage = surface * 0.82;

        vec3 ledColor = mix(bubble.body * 0.82, bubble.rim, rim * 0.46 + edge * 0.28);
        ledColor *= mix(0.72, 0.9, body) * mix(0.86, 1.0, edgeLight);
        color = mix(color, clamp(ledColor, 0.0, 1.0), coverage);
    }

    fragColor = vec4(clamp(color, 0.0, 1.0), 1.0);
}
