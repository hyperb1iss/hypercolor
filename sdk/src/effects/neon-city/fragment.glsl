#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iSpeed;
uniform float iDensity;
uniform float iGlow;
uniform float iRainIntensity;
uniform int iPalette;

float hash11(float p) {
    p = fract(p * 0.1031);
    p *= p + 33.33;
    p *= p + p;
    return fract(p);
}

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
    if (id == 1) return iqPalette(t, vec3(0.5, 0.2, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 1.0), vec3(0.8, 0.1, 0.6));
    if (id == 2) return iqPalette(t, vec3(0.3, 0.1, 0.4), vec3(0.5, 0.3, 0.5), vec3(0.8, 0.5, 0.7), vec3(0.9, 0.2, 0.4));
    if (id == 3) return iqPalette(t, vec3(0.5, 0.2, 0.0), vec3(0.5, 0.4, 0.2), vec3(1.0, 0.7, 0.4), vec3(0.0, 0.15, 0.2));
    if (id == 4) return iqPalette(t, vec3(0.2, 0.5, 0.4), vec3(0.3, 0.4, 0.4), vec3(0.8, 0.7, 0.9), vec3(0.6, 0.3, 0.7));
    return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
}

// Building silhouette
float building(vec2 uv, float x, float width, float height, float seed) {
    float left = x - width * 0.5;
    float right = x + width * 0.5;

    if (uv.x < left || uv.x > right || uv.y > height) return 0.0;

    // Rooftop variation
    float roofType = hash11(seed * 3.14);
    float h = height;
    if (roofType > 0.7) {
        // Antenna
        float antennaX = x + (hash11(seed * 5.0) - 0.5) * width * 0.3;
        if (abs(uv.x - antennaX) < 0.002 && uv.y < height + 0.06) {
            return 1.0;
        }
    }

    return 1.0;
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    float aspect = iResolution.x / iResolution.y;

    float time = iTime * iSpeed * 0.2;

    // Night sky gradient
    vec3 col = mix(
        vec3(0.02, 0.01, 0.05),
        vec3(0.05, 0.02, 0.08),
        uv.y
    );

    // Stars
    vec2 starUV = uv * vec2(aspect, 1.0) * 100.0;
    vec2 starCell = floor(starUV);
    float starRng = hash21(starCell);
    if (starRng > 0.97 && uv.y > 0.5) {
        vec2 starLocal = fract(starUV) - 0.5;
        float twinkle = 0.5 + 0.5 * sin(iTime * 2.0 + starRng * 40.0);
        col += vec3(0.5, 0.6, 0.8) * smoothstep(0.04, 0.0, length(starLocal)) * twinkle * 0.3;
    }

    // Parallax building layers
    float buildingMask = 0.0;
    vec3 neonGlow = vec3(0.0);

    for (int layer = 0; layer < 3; layer++) {
        float fl = float(layer);
        float depth = 1.0 - fl * 0.3;
        float parallaxShift = fl * 0.02 * sin(time * 0.3);

        float numBuildings = 8.0 + iDensity * 0.1 + fl * 3.0;
        float buildingScale = 0.8 - fl * 0.15;

        for (int b = 0; b < 20; b++) {
            if (float(b) >= numBuildings) break;
            float fb = float(b);
            float seed = fb + fl * 100.0;

            float bx = fb / numBuildings + parallaxShift;
            bx = fract(bx) * (1.0 + 0.2 / numBuildings) - 0.1;

            float bWidth = 0.03 + hash11(seed * 1.7) * 0.04;
            float bHeight = 0.15 + hash11(seed * 2.3) * 0.35 * buildingScale;

            float b_val = building(uv, bx, bWidth, bHeight, seed);

            if (b_val > 0.0) {
                // Building body — dark silhouette
                vec3 bColor = vec3(0.02, 0.015, 0.03) * depth;

                // Windows
                float windowCols = 2.0 + floor(hash11(seed * 4.0) * 4.0);
                float windowRows = floor(bHeight / 0.025);
                float wx = fract((uv.x - (bx - bWidth * 0.5)) / bWidth * windowCols);
                float wy = fract(uv.y / 0.025);

                float windowOn = step(0.6, hash21(vec2(
                    floor((uv.x - (bx - bWidth * 0.5)) / bWidth * windowCols),
                    floor(uv.y / 0.025)
                ) + seed));

                // Flickering
                float flicker = step(0.97, hash21(vec2(
                    floor(iTime * 0.5),
                    floor((uv.x - (bx - bWidth * 0.5)) / bWidth * windowCols) + floor(uv.y / 0.025)
                ) + seed));
                windowOn = max(windowOn, flicker) * (1.0 - flicker * 0.5);

                float windowShape = step(0.2, wx) * step(wx, 0.8) *
                                    step(0.3, wy) * step(wy, 0.7);

                vec3 windowColor = paletteColor(hash11(seed * 6.0 + floor(uv.y / 0.025)), iPalette);
                bColor += windowColor * windowShape * windowOn * 0.3;

                // Neon sign on some buildings
                if (hash11(seed * 7.0) > 0.5 && layer < 2) {
                    float signY = bHeight * 0.6;
                    float signH = 0.02;
                    if (uv.y > signY - signH && uv.y < signY + signH &&
                        uv.x > bx - bWidth * 0.4 && uv.x < bx + bWidth * 0.4) {

                        vec3 neonColor = paletteColor(hash11(seed * 8.0), iPalette);
                        float pulse = 0.7 + 0.3 * sin(iTime * 3.0 + seed * 5.0);
                        bColor += neonColor * pulse * iGlow * 0.015;

                        // Neon glow spread
                        float neonDist = abs(uv.y - signY);
                        neonGlow += neonColor * exp(-neonDist * 40.0) * iGlow * 0.003 * pulse;
                    }
                }

                if (layer == 0 || buildingMask < 0.5) {
                    col = bColor;
                }
                buildingMask = 1.0;
            }
        }
    }

    col += neonGlow;

    // Rain streaks
    if (iRainIntensity > 10.0) {
        float rainDensity = iRainIntensity * 0.005;
        vec2 rainUV = vec2(uv.x * 80.0, uv.y * 15.0 - time * 8.0);
        vec2 rainCell = floor(rainUV);
        float rainRng = hash21(rainCell);

        if (rainRng < rainDensity) {
            float rainLocal = fract(rainUV.y);
            float streak = smoothstep(0.0, 0.3, rainLocal) * smoothstep(1.0, 0.7, rainLocal);
            float rainX = fract(rainUV.x) - 0.5;
            streak *= smoothstep(0.02, 0.0, abs(rainX));
            col += vec3(0.3, 0.4, 0.6) * streak * 0.15;
        }
    }

    // Ground reflection
    if (uv.y < 0.05) {
        float reflectY = 0.1 - uv.y;
        float reflectFade = smoothstep(0.05, 0.0, reflectY);
        // Mirror the neon glow
        col += neonGlow * reflectFade * 0.3;
        // Wet ground specular
        col *= 1.0 + reflectFade * 0.5;
    }

    col = col / (1.0 + col * 0.3);
    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
