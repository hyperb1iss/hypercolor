#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

// Color
uniform int iPalette;
uniform float iIntensity;
uniform float iSaturation;
uniform int iColorShift;
uniform float iColorCycle;
uniform float iPulse;

// Motion
uniform float iSpeed;
uniform int iReverse;
uniform float iWave;

// Tunnel
uniform float iWidth;
uniform float iTexture;
uniform float iFog;

// Camera
uniform float iFov;
uniform float iTilt;

// Style
uniform int iStyle;

// ── Normalized globals (set in main) ─────────────────────────────

float normIntensity;
float normSaturation;
float normColorCycle;
float normPulse;
float normWave;
float normWidth;
float normTexture;
float normFog;
float normFov;
float normTilt;

float T;    // animation time (direction-aware)
float CT;   // color time (independent cycle speed)

// ── Color accumulator ────────────────────────────────────────────

vec3 rgb = vec3(0);

// ── Tunnel path ──────────────────────────────────────────────────

vec3 tunnelPath(float z) {
    float ampX = 12.0 * normWidth;
    float ampY = 24.0 * normWidth;
    return vec3(
        tanh(cos(z * 0.2 + sin(iTime * normWave) * 2.0 * normWave) * 0.4) * ampX,
        5.0 + tanh(cos(z * 0.14 + cos(iTime * normWave * 0.5) * 3.0 * normWave) * 0.5) * ampY,
        z
    );
}

mat2 rot(float a) {
    float c = cos(a), s = sin(a);
    return mat2(c, -s, s, c);
}

// ── Surface detail (triangular wave) ─────────────────────────────
// See "Xyptonjtroz" by nimitz — shadertoy.com/view/4ts3z2

vec3 tri(vec3 x) {
    return abs(x - floor(x) - 0.5);
}

// ── Color utilities ──────────────────────────────────────────────

vec3 hsv2rgb(vec3 c) {
    vec4 K = vec4(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    vec3 p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
    return c.z * mix(K.xxx, clamp(p - K.xxx, 0.0, 1.0), c.y);
}

vec3 rgb2hsv(vec3 c) {
    vec4 K = vec4(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
    vec4 p = mix(vec4(c.bg, K.wz), vec4(c.gb, K.xy), step(c.b, c.g));
    vec4 q = mix(vec4(p.xyw, c.r), vec4(c.r, p.yzx), step(p.x, c.r));
    float d = q.x - min(q.w, q.y);
    float e = 1.0e-10;
    return vec3(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
}

vec3 blendOverlay(vec3 base, vec3 blend) {
    return mix(
        2.0 * base * blend,
        1.0 - 2.0 * (1.0 - base) * (1.0 - blend),
        step(0.5, base)
    );
}

vec3 blendSoftLight(vec3 base, vec3 blend) {
    return mix(
        2.0 * base * blend + base * base * (1.0 - 2.0 * blend),
        sqrt(base) * (2.0 * blend - 1.0) + 2.0 * base * (1.0 - blend),
        step(0.5, blend)
    );
}

vec3 limitWhiteness(vec3 color, float threshold) {
    float brightness = max(max(color.r, color.g), color.b);
    if (brightness > threshold) {
        vec3 hsv = rgb2hsv(color);
        hsv.z = mix(hsv.z, threshold, smoothstep(threshold, threshold + 0.2, hsv.z));
        hsv.y = min(1.0, hsv.y * 1.2);
        return hsv2rgb(hsv);
    }
    return color;
}

vec3 saturateColor(vec3 color, float factor) {
    vec3 hsv = rgb2hsv(color);
    hsv.y = clamp(hsv.y * factor, 0.0, 1.0);
    if (factor > 1.0) {
        hsv.z = hsv.z / (1.0 + (factor - 1.0) * 0.15);
    }
    hsv.z = max(hsv.z, 0.05);
    return hsv2rgb(hsv);
}

// ── Color palettes ───────────────────────────────────────────────
// 16 schemes, indexed by iPalette combo

vec3 getColorPalette(int scheme, vec3 baseColor) {
    // Multi-frequency color pulse
    float pulseFactor = 1.0;
    if (normPulse > 0.0) {
        float fastPulse = sin(CT * 5.0) * 0.5 + 0.5;
        float slowPulse = sin(CT * 2.0) * 0.5 + 0.5;
        float weirdPulse = sin(CT * 0.7) * sin(CT * 1.3) * 0.5 + 0.5;
        float mixedPulse = mix(
            mix(fastPulse, slowPulse, 0.5),
            weirdPulse,
            sin(CT * 0.3) * 0.5 + 0.5
        );
        pulseFactor = 1.0 + (mixedPulse - 0.5) * normPulse * 0.8;
    }

    vec3 baseHSV = rgb2hsv(baseColor);
    float intensityFactor = max(0.01, normIntensity) * pulseFactor;
    float depthEffect = sin(baseHSV.x * 10.0 + CT) * 0.1;
    float spatialEffect = sin(baseHSV.z * 8.0 + CT * 0.5) * 0.15;

    baseHSV.x += depthEffect;

    if (iColorShift == 1) {
        float depthCoord = baseHSV.z * 3.0 + CT * 0.1;
        float hueShift = sin(depthCoord) * sin(depthCoord * 0.7) * 0.07;
        float smoothFactor = smoothstep(0.0, 1.0, sin(CT * 0.15) * 0.5 + 0.5);
        hueShift *= smoothFactor;
        baseHSV.x = fract(baseHSV.x + hueShift);
    }

    baseHSV.y = min(1.0, baseHSV.y * (1.0 + spatialEffect));
    baseHSV.z = pow(baseHSV.z, 0.5) * intensityFactor;
    vec3 enhanced = hsv2rgb(baseHSV);

    // 0: Sapphire — luminous deep blue with teal highlights
    vec3 color = enhanced * vec3(0.5, 0.8, 1.3);

    if (scheme == 1) {
        // Cyberpunk — purple / teal neon bleed
        float shift = sin(CT * 0.2) * 0.2;
        color = enhanced * vec3(1.0 + shift, 0.25, 1.4 - shift)
              + vec3(0.03, 0.05 + sin(CT * 0.7) * 0.03, 0.25);
    } else if (scheme == 2) {
        // Inferno — molten reds, flickering orange glow
        float flicker = sin(CT * 8.0) * sin(CT * 5.7) * 0.1;
        float glow = sin(CT * 0.4) * 0.2 + 0.8;
        color = enhanced * vec3(1.9 * glow, 0.4 + flicker, 0.05)
              + vec3(flicker * 0.2, 0.0, 0.0);
    } else if (scheme == 3) {
        // Toxic — pulsing acid greens and yellows
        float toxicPulse = sin(CT * 1.2) * 0.15 + 0.85;
        float yellowShift = cos(CT * 0.7) * 0.3;
        color = enhanced * vec3(0.25 + yellowShift, 1.6 * toxicPulse, 0.35)
              + vec3(sin(CT * 3.1) * 0.05, 0.0, 0.0);
    } else if (scheme == 4) {
        // Ethereal — shifting pastels, dreamy luminosity
        float eShift = sin(CT * 0.3);
        float blueShift = cos(CT * 0.5) * 0.2;
        color = enhanced * vec3(0.7 + eShift * 0.1, 0.9, 1.3 - blueShift)
              + vec3(0.25 + sin(CT * 1.1) * 0.1, eShift * 0.1, 0.4 + blueShift * 0.2);
    } else if (scheme == 5) {
        // Monochrome — silver ghost with subtle tint drift
        float tint = sin(CT * 0.2) * 0.05;
        float luminance = dot(enhanced, vec3(0.299, 0.587, 0.114));
        color = vec3(luminance * 1.6) + vec3(tint, tint, tint * 1.5);
    } else if (scheme == 6) {
        // Spectrum — full rainbow cycling with spatial variation
        float hueBase = CT * 0.1;
        float hueSpatial = sin(enhanced.x * 5.0 + enhanced.y * 3.0) * 0.2;
        vec3 rainbow;
        rainbow.r = sin(hueBase + hueSpatial) * 0.5 + 0.7;
        rainbow.g = sin(hueBase + 2.0 + hueSpatial) * 0.5 + 0.7;
        rainbow.b = sin(hueBase + 4.0 + hueSpatial) * 0.5 + 0.7;
        float rainbowPulse = sin(CT * 2.0) * 0.1 + 0.9;
        color = enhanced * (rainbow * rainbowPulse) + vec3(0.15);
    } else if (scheme == 7) {
        // Electric — crackling blues with lightning flashes
        float flash = pow(sin(CT * 10.0) * 0.5 + 0.5, 4.0) * sin(CT * 5.0);
        float glow = sin(CT * 0.5) * 0.2 + 0.8;
        color = enhanced * vec3(0.2, 0.7, 1.8 * glow)
              + vec3(0.3 * sin(CT * 0.3))
              + vec3(flash);
        if (sin(CT * 0.73) > 0.98 || flash > 0.85) {
            color += vec3(0.2, 0.3, 0.6) * flash * (1.0 + normPulse);
        }
    } else if (scheme == 8) {
        // Amethyst — rich purples with crystalline shimmer
        float shimmer = pow(sin(CT * 7.0 + enhanced.x * 20.0) * 0.5 + 0.5, 8.0) * 0.2;
        float purpleShift = sin(CT * 0.3) * 0.1;
        color = enhanced * vec3(0.8 + purpleShift, 0.3, 1.2 - purpleShift)
              + vec3(shimmer)
              + vec3(0.05, 0.0, 0.1);
    } else if (scheme == 9) {
        // Coral — vibrant underwater reef colors
        float waveMotion = sin(CT * 0.5 + enhanced.y * 3.0) * 0.1;
        float blueOverlay = sin(CT * 0.2) * 0.1 + 0.2;
        color = enhanced * vec3(1.3 + waveMotion, 0.8 - waveMotion, 0.4)
              + vec3(0.0, 0.1, blueOverlay);
        if (sin(enhanced.x * 10.0 + CT) > 0.8) {
            color += vec3(0.15, 0.05, 0.0);
        }
    } else if (scheme == 10) {
        // Abyss — deep ocean with bioluminescent sparks
        float luminescence = pow(sin(enhanced.z * 8.0 + CT) * 0.5 + 0.5, 4.0) * 0.4;
        float depth = sin(CT * 0.1) * 0.1 + 0.8;
        color = enhanced * vec3(0.1, 0.3, depth)
              + vec3(0.0, luminescence * 0.6, luminescence);
        if (fract(sin(dot(vec2(enhanced.x, enhanced.y * CT * 0.1), vec2(12.9898, 78.233))) * 43758.5453) > 0.99) {
            color += vec3(0.0, 0.2, 0.3) * (sin(CT * 2.0) * 0.5 + 0.5);
        }
    } else if (scheme == 11) {
        // Emerald — deep greens with crystal-like reflections
        float crystalFlash = pow(sin(enhanced.y * 10.0 + CT * 2.0) * 0.5 + 0.5, 6.0) * 0.3;
        float greenShift = sin(CT * 0.4) * 0.1;
        color = enhanced * vec3(0.2, 1.1 + greenShift, 0.5)
              + vec3(crystalFlash * 0.7, crystalFlash, crystalFlash * 0.6);
    } else if (scheme == 12) {
        // Neon — vibrant shifting hues on dark backdrop
        float neonPulse = sin(CT * 1.5) * 0.15 + 0.85;
        float hueShift = fract(enhanced.z * 0.2 + CT * 0.05);
        float hueSector = floor(hueShift * 3.0);
        vec3 neonColor;
        if (hueSector < 1.0)      neonColor = vec3(1.0, 0.1, 0.8);
        else if (hueSector < 2.0) neonColor = vec3(0.1, 1.0, 0.8);
        else                      neonColor = vec3(0.9, 0.8, 0.1);
        color = enhanced * neonColor * neonPulse + vec3(0.05);
        color = mix(vec3(0.02, 0.02, 0.05), color, min(1.0, color.r + color.g + color.b));
    } else if (scheme == 13) {
        // Rose Gold — warm metallic pinks and golds
        float metallic = pow(sin(enhanced.x * 5.0 + enhanced.y * 3.0 + CT) * 0.5 + 0.5, 2.0);
        float warmth = sin(CT * 0.3) * 0.1 + 0.9;
        color = enhanced * vec3(1.1 * warmth, 0.7, 0.6)
              + vec3(metallic * 0.3, metallic * 0.2, metallic * 0.1);
    } else if (scheme == 14) {
        // Sunset — warm oranges through deep purples
        float skyGradient = enhanced.y * 2.0;
        float sunsetPhase = sin(CT * 0.2) * 0.5 + 0.5;
        vec3 horizon = mix(vec3(1.5, 0.6, 0.2), vec3(0.9, 0.2, 0.5), sunsetPhase);
        vec3 zenith = mix(vec3(0.7, 0.3, 0.9), vec3(0.2, 0.2, 0.8), sunsetPhase);
        color = enhanced * mix(horizon, zenith, skyGradient);
        float sun = pow(max(0.0, 1.0 - length(enhanced.xy * 2.0)), 5.0);
        color += vec3(1.0, 0.6, 0.2) * sun * 0.5;
    } else if (scheme == 15) {
        // Vaporwave — retro 80s pink/cyan with glitch lines
        float gridEffect = max(0.0, sin(enhanced.x * 10.0 + CT) * sin(enhanced.y * 10.0 + CT) - 0.8) * 0.5;
        float vaporShift = sin(CT * 0.2) * 0.1;
        color = enhanced * vec3(0.9 + vaporShift, 0.4, 0.9 - vaporShift)
              + vec3(0.1, 0.1 + gridEffect, 0.3);
        if (fract(CT * 2.0) < 0.03) {
            float glitchPos = floor(sin(CT * 10.0) * 10.0) / 10.0;
            if (abs(enhanced.y - glitchPos) < 0.02) {
                color *= vec3(0.8, 1.2, 1.5);
            }
        }
    }

    // Dynamic post-processing
    vec3 resultHSV = rgb2hsv(color);
    float satMod = sin(CT * 0.4 + resultHSV.x * 10.0) * 0.15;
    resultHSV.y = clamp(resultHSV.y + satMod, 0.0, 1.0);
    resultHSV.x += sin(CT * 0.1 + resultHSV.z * 3.0) * 0.02;
    color = hsv2rgb(resultHSV);

    color = pow(color, vec3(0.95));
    color = mix(color, blendSoftLight(color, vec3(0.7, 0.8, 0.9)), 0.5);
    color = limitWhiteness(color, 0.8);

    return color;
}

// ── Visual styles ────────────────────────────────────────────────

vec4 applyStyle(vec4 color, float depth) {
    // Anti-banding dither when color shift is active
    if (iColorShift == 1) {
        vec2 uv = gl_FragCoord.xy / iResolution.xy;
        float dither = fract(sin(dot(uv, vec2(12.9898, 78.233))) * 43758.5453) * 0.01 - 0.005;
        color.rgb += vec3(dither);
    }

    if (iStyle == 1) {
        // Chromatic Shift — RGB channel separation along tunnel motion
        // Offsets UVs per channel to simulate light splitting through the void
        vec2 uv = gl_FragCoord.xy / iResolution.xy;
        float speed = length(color.rgb) * 0.3 + 0.1;
        float aberration = (0.003 + speed * 0.004) * (1.0 + sin(iTime * 2.0) * 0.3);
        vec2 dir = normalize(uv - 0.5);
        float dist = length(uv - 0.5);
        aberration *= dist;  // stronger toward edges

        // We can't re-render per channel, so shift color balance directionally
        float rShift = sin(dot(dir, vec2(1.0, 0.0)) * 6.0 + iTime * 3.0) * aberration;
        float bShift = sin(dot(dir, vec2(-1.0, 0.0)) * 6.0 + iTime * 3.0) * aberration;
        color.r *= 1.0 + rShift * 15.0;
        color.b *= 1.0 + bShift * 15.0;
        color.g *= 1.0 - abs(rShift + bShift) * 5.0;
        return color;
    } else if (iStyle == 2) {
        // Phosphor — CRT phosphor decay with sub-pixel color separation
        vec2 px = gl_FragCoord.xy;
        float subpixel = mod(px.x, 3.0);
        vec3 mask = vec3(
            smoothstep(0.0, 1.0, 1.0 - abs(subpixel - 0.5)),
            smoothstep(0.0, 1.0, 1.0 - abs(subpixel - 1.5)),
            smoothstep(0.0, 1.0, 1.0 - abs(subpixel - 2.5))
        );
        // Scanline with depth-dependent intensity
        float scanGap = sin(px.y * 1.5) * 0.5 + 0.5;
        float scanIntensity = mix(0.15, 0.05, exp(-depth * 0.05));
        float scan = 1.0 - scanGap * scanIntensity;
        color.rgb *= mix(vec3(1.0), mask * 1.4 + 0.3, 0.4) * scan;
        // Subtle bloom on bright pixels
        float brightness = dot(color.rgb, vec3(0.299, 0.587, 0.114));
        color.rgb += color.rgb * max(0.0, brightness - 0.6) * 0.3;
        return color;
    }

    return color;
}

// ── Surface detail ───────────────────────────────────────────────

float triSurface(vec3 p) {
    float waveEffect = 0.0;
    if (normWave > 0.0) {
        waveEffect = sin(p.z * 0.2 + iTime * 2.0) * normWave * 0.5;
    }

    if (normTexture < 0.1) return 0.0;

    vec3 p1 = p + waveEffect;
    vec3 p2 = p * 0.2 + waveEffect + tri(0.05 * T + p1) * normTexture;
    float surface = dot(tri(0.15 * T + p * 0.25 + tri(p2)) + tri(p1) * 0.2, vec3(2.5));
    return (1.0 - surface) * normTexture;
}

// ── SDF ──────────────────────────────────────────────────────────

float map(vec3 p) {
    float a;
    float s = 1.5 - min(length(p.xy - tunnelPath(p.z).xy), p.y - tunnelPath(p.z).x);
    s = min(6.5 + p.y, s);
    s -= triSurface(p);

    for (a = 0.1; a < 1.0; s -= abs(dot(sin(T + p * a * 40.0), vec3(0.01))) / a, a += a * 1.5);

    rgb += sin(p) * 0.15 + 0.175;

    if (iColorShift == 1) {
        vec3 colorVar = vec3(
            sin(p.x * 0.2 + p.z * 0.1 + iTime * 0.23),
            sin(p.y * 0.2 + p.z * 0.12 + iTime * 0.19),
            sin(p.z * 0.2 + p.x * 0.11 + iTime * 0.17)
        ) * 0.02;
        colorVar = smoothstep(-0.02, 0.02, colorVar) * 0.03;
        rgb += colorVar;
    }

    return s;
}

// ── Entry ────────────────────────────────────────────────────────

void main() {
    // Normalize SDK control ranges to shader-internal values
    normIntensity  = max(0.01, iIntensity * 0.02);   // 0-100 → 0-2
    normSaturation = max(0.01, iSaturation * 0.02);   // 0-100 → 0-2
    normColorCycle = iColorCycle * 0.02;               // 0-100 → 0-2
    normPulse      = iPulse * 0.01;                    // 0-100 → 0-1
    normWave       = iWave * 0.01;                     // 0-100 → 0-1
    normWidth      = max(0.2, iWidth * 0.02);          // 0-100 → 0.2-2
    normTexture    = iTexture * 0.02;                  // 0-100 → 0-2
    normFog        = max(0.2, iFog * 0.02);            // 0-100 → 0.2-2
    normFov        = max(0.2, iFov * 0.02);            // 0-100 → 0.2-2
    normTilt       = iTilt * 0.02;                     // 0-100 → 0-2

    float speed = max(iSpeed, 0.5);
    T  = iTime * 3.5 * speed * (iReverse == 1 ? -1.0 : 1.0);
    CT = iTime * normColorCycle * 2.0;

    // Camera setup
    float fov = 0.5 + normFov * 0.75;
    float tiltAngle = sin(T * 0.2) * 0.3 * normTilt * 0.5;

    vec2 fc = gl_FragCoord.xy;
    vec3 r = vec3(iResolution, 0.0);
    vec3 p = tunnelPath(T);
    vec3 ro = p;
    vec3 Z = normalize(tunnelPath(T + 3.0) - p);
    vec3 X = normalize(vec3(Z.z, 0, -Z.x));
    vec3 D = vec3(rot(tiltAngle) * (fc - r.xy / 2.0) / r.y * fov, 1)
           * mat3(-X, cross(X, Z), Z);

    rgb = vec3(0);

    // Ray march
    float s = 0.002, d = 0.0, i = 0.0;
    while (i++ < 60.0 && s > 0.001 && d < 100.0) {
        p = ro + D * d;
        d += s = map(p) * 0.4;
    }

    // Ensure color sampling varies across screen even when camera is near geometry
    // (prevents solid-color frames when march terminates at d ≈ 0)
    vec3 colorP = ro + D * max(d, 0.5);

    // Color detail accumulation
    float a;
    for (a = 0.5; a < 4.0; rgb += abs(dot(sin(colorP * a * 8.0), vec3(0.07))) / a, a *= 1.6);

    // Apply palette (expects raw accumulated range — don't compress beforehand)
    rgb = getColorPalette(iPalette, rgb);

    // Fog attenuation
    float fogScale = 2.0 + (2.0 - normFog) * 3.0;
    float distAtten = mix(1.0, exp(-d / fogScale), 0.85);
    vec3 rawColor = rgb * distAtten + 0.03;

    // Tone map after palette to cap highlights without changing palette character
    rawColor = rawColor / (1.0 + rawColor);

    vec3 baseColor = pow(rawColor, vec3(0.5));

    // Saturation + whiteness control
    baseColor = saturateColor(baseColor, normSaturation);
    baseColor = limitWhiteness(baseColor, 0.9);
    baseColor = blendSoftLight(baseColor, baseColor * vec3(0.95, 1.0, 1.05));

    // Apply visual style
    fragColor = applyStyle(vec4(baseColor, 1.0), d);
}
