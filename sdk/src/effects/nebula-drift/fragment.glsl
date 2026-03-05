#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iSpeed;
uniform float iDensity;
uniform float iStarSize;
uniform float iGlow;
uniform int iDirection;
uniform int iBackground;
uniform int iStarMode;
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

vec2 hash22(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * vec3(0.1031, 0.1030, 0.0973));
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.xx + p3.yz) * p3.zy);
}

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return iqPalette(t, vec3(0.44, 0.20, 0.55), vec3(0.56, 0.55, 0.48), vec3(1.0, 0.9, 0.7), vec3(0.84, 0.16, 0.52));
    if (id == 1) return iqPalette(t, vec3(0.18, 0.28, 0.54), vec3(0.32, 0.44, 0.52), vec3(0.86, 0.95, 1.0), vec3(0.72, 0.08, 0.22));
    if (id == 2) return iqPalette(t, vec3(0.48, 0.28, 0.08), vec3(0.52, 0.42, 0.28), vec3(1.0, 0.9, 0.6), vec3(0.06, 0.20, 0.28));
    if (id == 3) return iqPalette(t, vec3(0.18, 0.42, 0.35), vec3(0.30, 0.34, 0.40), vec3(0.78, 0.82, 0.95), vec3(0.52, 0.24, 0.65));
    if (id == 4) {
        float m = 0.65 + 0.35 * sin(6.28318 * t);
        return vec3(m);
    }
    return iqPalette(t, vec3(0.44, 0.20, 0.55), vec3(0.56, 0.55, 0.48), vec3(1.0, 0.9, 0.7), vec3(0.84, 0.16, 0.52));
}

vec2 flowDirection(vec2 p, int mode) {
    if (mode == 0 || mode == 1) {
        vec2 radial = normalize(p + vec2(0.0001, 0.0));
        vec2 tangent = vec2(-radial.y, radial.x);
        float signDir = (mode == 0) ? 1.0 : -1.0;
        float swirl = 0.08 + smoothstep(0.0, 1.25, length(p)) * 0.16;
        return normalize(radial * signDir + tangent * swirl * signDir);
    }
    if (mode == 2) return vec2(-1.0, 0.0);
    if (mode == 3) return vec2(1.0, 0.0);
    if (mode == 4) return vec2(0.0, 1.0);
    return vec2(0.0, -1.0);
}

vec3 backgroundColor(vec2 uv, vec2 p, float time, int mode, int paletteId) {
    float r = length(p);
    float horizon = smoothstep(-0.65, 0.85, uv.y);

    vec3 low = paletteColor(0.08 + uv.x * 0.05 + time * 0.012, paletteId) * 0.045;
    vec3 high = paletteColor(0.38 + uv.y * 0.06 - time * 0.010, paletteId) * 0.085;
    vec3 col = mix(low, high, horizon);

    if (mode == 1) {
        // Cockpit: subtle forward glow with lower rim.
        float rim = smoothstep(0.05, 1.10, r);
        float lane = exp(-pow((uv.y + 0.35) * 2.2, 2.0));
        col += paletteColor(0.62 + uv.x * 0.09, paletteId) * rim * 0.040;
        col += paletteColor(0.22 + uv.x * 0.03, paletteId) * lane * 0.035;
    } else if (mode == 2) {
        // Wormhole: radial bands and angular lane hints.
        float angle = atan(p.y, p.x) / 6.28318;
        float rings = sin(r * 40.0 - time * 5.2);
        float lanes = sin(angle * 36.0 + time * 1.6);
        col += paletteColor(0.18 + r * 0.24, paletteId) * smoothstep(0.72, 1.0, rings) * exp(-r * 1.4) * 0.095;
        col += paletteColor(0.72 + angle * 0.30, paletteId) * smoothstep(0.82, 1.0, lanes) * exp(-r * 1.0) * 0.042;
    } else if (mode == 3) {
        // Grid void: faint moving lattice.
        vec2 g = abs(fract((p + vec2(time * 0.06, 0.0)) * 8.2) - 0.5);
        float grid = smoothstep(0.495, 0.445, max(g.x, g.y));
        col += paletteColor(0.54 + uv.x * 0.10, paletteId) * grid * 0.040;
    }

    float vignette = smoothstep(1.45, 0.20, r);
    col *= 0.42 + vignette * 0.68;
    return col;
}

vec2 streakProfile(vec2 q, vec2 flow, float sizeNorm, float speedNorm, int shape) {
    vec2 dir = normalize(flow + vec2(0.0001, 0.0));
    vec2 side = vec2(-dir.y, dir.x);

    float along = dot(q, dir);
    float across = dot(q, side);

    float width = mix(0.006, 0.030, sizeNorm);
    float length = mix(0.12, 0.62, sizeNorm) * (0.65 + speedNorm * 1.25);

    if (shape == 0) {
        // Needles
        length *= 1.35;
        width *= 0.58;
    } else if (shape == 1) {
        // Comets
        length *= 0.92;
        width *= 1.15;
    } else {
        // Shards
        length *= 1.08;
        width *= 0.82;
    }

    float tail = exp(-max(-along, 0.0) * (6.0 / max(length, 0.04)));
    tail *= smoothstep(width * 1.1, 0.0, abs(across));
    tail *= smoothstep(-length, 0.0, along);

    float head = exp(
        -(along * along) * (900.0 / (1.0 + sizeNorm * 4.0)) -
        (across * across) * (1900.0 / (1.0 + sizeNorm * 5.0))
    );

    float core = head * 1.65 + tail;

    if (shape == 2) {
        float shardA = exp(-abs(across) * (44.0 - sizeNorm * 22.0)) * exp(-abs(along) * (11.0 + (1.0 - speedNorm) * 8.0));
        float shardB = exp(-abs(across + along * 0.62) * 33.0) * exp(-abs(along) * 10.0);
        core += shardA * 0.62 + shardB * 0.32;
    }

    float bloom = exp(
        -(along * along) * (90.0 / (1.0 + sizeNorm * 2.5)) -
        (across * across) * (130.0 / (1.0 + sizeNorm * 3.0))
    );

    if (shape == 0) bloom *= 0.55;
    if (shape == 2) bloom *= 0.75;

    return vec2(core, bloom);
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    vec2 p = uv * 2.0 - 1.0;
    p.x *= iResolution.x / iResolution.y;

    float speedNorm = clamp(iSpeed / 2.8, 0.0, 1.0);
    float densityNorm = clamp(iDensity * 0.01, 0.10, 1.0);
    float sizeNorm = clamp(iStarSize * 0.01, 0.05, 1.0);
    float glowNorm = clamp(iGlow * 0.01, 0.0, 1.0);

    int directionMode = clamp(iDirection, 0, 5);
    int backgroundMode = clamp(iBackground, 0, 3);
    int starMode = clamp(iStarMode, 0, 3);

    float time = iTime * (0.35 + iSpeed * 0.90);
    vec3 col = backgroundColor(uv, p, time, backgroundMode, iPalette);

    float spawnBase = mix(0.996, 0.84, densityNorm);
    float sparkleTime = iTime * (1.3 + speedNorm * 2.4);

    for (int layer = 0; layer < 3; layer++) {
        float fl = float(layer);
        vec2 layerP = p * (1.0 + fl * 0.30);
        vec2 flow = flowDirection(layerP, directionMode);

        float cellScale = (20.0 + fl * 18.0) + densityNorm * (16.0 + fl * 12.0);
        float layerSpeed = (0.55 + fl * 0.50) * (0.70 + speedNorm * 2.0);

        vec2 grid = layerP * cellScale + flow * time * layerSpeed;
        vec2 baseCell = floor(grid);
        vec2 local = fract(grid) - 0.5;

        float spawn = spawnBase - fl * 0.018;

        for (int oy = -1; oy <= 1; oy++) {
            for (int ox = -1; ox <= 1; ox++) {
                vec2 neighbor = vec2(float(ox), float(oy));
                vec2 cell = baseCell + neighbor;

                float seed = hash21(cell + vec2(17.0 * fl + 3.7, 51.0));
                if (seed <= spawn) continue;

                vec2 jitter = (hash22(cell + seed * 41.7) - 0.5) * 0.76;
                vec2 q = local - neighbor - jitter;

                int shape = starMode;
                if (starMode == 3) {
                    float pick = hash21(cell + vec2(31.0 + fl * 13.0, 9.0));
                    if (pick < 0.34) shape = 0;
                    else if (pick < 0.68) shape = 1;
                    else shape = 2;
                }

                vec2 streak = streakProfile(q, flow, sizeNorm, speedNorm, shape);

                float twinkle = 0.55 + 0.45 * sin(sparkleTime * (1.0 + seed * 1.4) + seed * 70.0 + fl * 3.2);
                float core = streak.x * (0.30 + twinkle * 0.90) * (0.55 + fl * 0.28);
                float bloom = streak.y * glowNorm * (0.16 + fl * 0.12);

                float tone = fract(seed * 1.73 + fl * 0.21 + time * 0.02);
                vec3 tint = paletteColor(0.12 + tone, iPalette);
                vec3 hot = mix(vec3(1.0, 0.98, 0.92), tint, 0.55);

                col += hot * core;
                col += tint * bloom;
            }
        }
    }

    if (directionMode <= 1) {
        // Radial tunnel emphasis for forward/reverse modes.
        float r = length(p);
        float centerPulse = exp(-r * (4.5 - speedNorm * 1.8));
        float rings = sin(r * (58.0 + speedNorm * 28.0) - time * (8.0 + speedNorm * 6.0));
        float ringMask = smoothstep(0.70, 1.0, rings);
        vec3 tunnelTint = paletteColor(0.74 + r * 0.25 - time * 0.03, iPalette);

        col += tunnelTint * centerPulse * (0.08 + glowNorm * 0.20);
        col += tunnelTint * ringMask * centerPulse * glowNorm * 0.08;
    } else {
        // Lateral flight lanes for non-radial modes.
        vec2 flow = flowDirection(p, directionMode);
        float lane = dot(p, vec2(-flow.y, flow.x));
        float laneGlow = exp(-lane * lane * 8.0) * (0.03 + glowNorm * 0.06);
        col += paletteColor(0.42 + p.x * 0.08 - p.y * 0.05, iPalette) * laneGlow;
    }

    float vignette = smoothstep(1.45, 0.16, length(p));
    col *= 0.36 + 0.74 * vignette;

    col = col / (1.0 + col * 0.38);
    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
