#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform int iPalette;
uniform float iSpeed;
uniform float iDensity;
uniform float iStreak;
uniform float iWarp;

// ─── Hash primitives ────────────────────────────────────────────────

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

// ─── Palette ────────────────────────────────────────────────────────

// Returns (core color, streak tint) packed as two vec3s via out param
void streakColors(int id, float laneHash, out vec3 core, out vec3 tint) {
    if (id == 0) {
        // Classic — pure white core, faint blue tails
        core = vec3(1.0, 1.0, 1.0);
        tint = vec3(0.27, 0.53, 1.0);
        return;
    }
    if (id == 1) {
        // Cyberpunk — white core, alternating cyan/magenta
        core = vec3(1.0, 1.0, 1.0);
        tint = laneHash > 0.5
            ? vec3(0.0, 1.0, 1.0)
            : vec3(1.0, 0.0, 1.0);
        return;
    }
    if (id == 2) {
        // Phantom Gate — white core, pale green to teal
        core = vec3(1.0, 1.0, 1.0);
        tint = mix(vec3(0.53, 1.0, 0.67), vec3(0.0, 0.80, 0.67), laneHash);
        return;
    }
    if (id == 3) {
        // Solar Wind — yellow-white core, gold to amber
        core = vec3(1.0, 0.97, 0.85);
        tint = mix(vec3(1.0, 0.67, 0.0), vec3(1.0, 0.40, 0.0), laneHash);
        return;
    }
    if (id == 4) {
        // Void — dim white core, deep red streaks
        core = vec3(0.75, 0.72, 0.70);
        tint = vec3(1.0, 0.13, 0.0);
        return;
    }
    // 5: Warp Core — white core, blue-to-purple gradient
    core = vec3(1.0, 1.0, 1.0);
    tint = mix(vec3(0.13, 0.40, 1.0), vec3(0.53, 0.27, 1.0), laneHash);
}

// ─── Star Layer ─────────────────────────────────────────────────────

// Renders a single depth layer of radial star streaks.
// Returns accumulated color for this layer.
vec3 starLayer(
    vec2 p,
    float radius,
    float angle,
    float time,
    float numLanes,
    float speedMul,
    float streakFactor,
    float brightness,
    float warpAmount,
    int palette
) {
    vec3 accum = vec3(0.0);
    float cellWidth = 6.2831853 / numLanes;

    // Keep the loop bound constant for GLSL ES while leaving headroom for density overdrive.
    for (float i = 0.0; i < 96.0; i += 1.0) {
        if (i >= numLanes) break;

        float cellAngle = i * cellWidth;
        float laneHash = hash11(i * 73.156 + numLanes * 1.7);
        float jitter = (laneHash - 0.5) * cellWidth * 0.7;
        float starAngle = cellAngle + jitter;

        // Warp: add spiral twist proportional to radius
        float spiralTwist = warpAmount * radius * 2.5;

        // Star radial position — cycles outward over time
        float phase = fract(hash11(i * 37.91 + numLanes * 3.1) + time * speedMul);

        // Perspective acceleration: stars appear faster as they move out
        float starR = phase * phase; // quadratic acceleration
        float maxR = 1.2;
        starR *= maxR;

        // Streak length grows with distance from center
        float sLen = starR * streakFactor * (0.3 + phase * 0.7);

        // Star direction with warp
        float sAngle = starAngle + spiralTwist * starR;
        vec2 starDir = vec2(cos(sAngle), sin(sAngle));
        vec2 starPos = starDir * starR;

        // Distance from pixel to the streak line segment
        vec2 toPixel = p - starPos;
        float along = dot(toPixel, starDir);
        float clamped = clamp(along, -0.002, sLen);
        vec2 closest = starPos + starDir * clamped;
        float perp = length(p - closest);

        // Brightness: hot near the star head, fading along the streak
        float streakT = clamp(along / max(sLen, 0.001), 0.0, 1.0);

        // Core glow — inverse square with floor
        float glow = brightness / (perp * perp * 800.0 + 0.0004);

        // Fade stars near center (they're "approaching") and at the edge (wrapping)
        float radialFade = smoothstep(0.0, 0.08, starR) * smoothstep(maxR, maxR * 0.85, starR);
        glow *= radialFade;

        // Size variation per star
        float sizeMul = 0.5 + laneHash * 0.8;
        glow *= sizeMul;

        // Color: white core transitioning to palette tint along streak
        vec3 coreColor, tintColor;
        streakColors(palette, laneHash, coreColor, tintColor);

        float tintMix = smoothstep(0.0, 0.6, streakT) * 0.7;
        vec3 starColor = mix(coreColor, tintColor, tintMix);

        // Subtle brightness pulsing per star
        float pulse = 0.85 + 0.15 * sin(time * 3.0 + laneHash * 40.0);
        glow *= pulse;

        accum += starColor * glow;
    }

    return accum;
}

// ─── Main ───────────────────────────────────────────────────────────

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    vec2 p = (uv - 0.5) * vec2(iResolution.x / iResolution.y, 1.0);

    float speed = max(iSpeed, 0.5);
    // Preserve the original 0-100 response, then use the 100-160 band as overdrive.
    float density = clamp(iDensity * 0.01, 0.0, 1.0);
    float densityOverdrive = smoothstep(0.0, 1.0, clamp((iDensity - 100.0) / 60.0, 0.0, 1.0));
    float streak = clamp(iStreak * 0.01, 0.0, 1.0);
    float streakOverdrive = smoothstep(0.0, 1.0, clamp((iStreak - 100.0) / 60.0, 0.0, 1.0));
    float warp = clamp(iWarp * 0.01, 0.0, 1.0);

    float time = iTime * (0.08 + speed * 0.08);

    float radius = length(p);
    float angle = atan(p.y, p.x);

    // ── Background ──
    // Near-black with barely perceptible warm radial glow toward center
    float bgGlow = exp(-radius * 4.0) * 0.03;
    vec3 color = vec3(bgGlow * 0.4, bgGlow * 0.3, bgGlow * 0.6);

    float layer0Lanes = mix(30.0, 60.0, density) + densityOverdrive * 24.0;
    float layer1Lanes = mix(15.0, 30.0, density) + densityOverdrive * 12.0;
    float layer2Lanes = mix(8.0, 18.0, density) + densityOverdrive * 8.0;

    float layer0Streak = mix(0.02, 0.08, streak) + streakOverdrive * 0.04;
    float layer1Streak = mix(0.05, 0.18, streak) + streakOverdrive * 0.10;
    float layer2Streak = mix(0.10, 0.35, streak) + streakOverdrive * 0.30;

    // ── Layer 0: Background stars — many, dim, slow ──
    vec3 layer0 = starLayer(
        p, radius, angle, time,
        layer0Lanes,
        0.35,                          // slow speed
        layer0Streak,                  // short streaks with overdrive headroom
        0.0006,                        // dim
        warp,
        iPalette
    );

    // ── Layer 1: Midground stars — medium count, medium speed ──
    vec3 layer1 = starLayer(
        p, radius, angle, time * 1.1 + 17.3,
        layer1Lanes,
        0.6,                           // medium speed
        layer1Streak,                  // medium streaks with overdrive headroom
        0.0015,                        // medium brightness
        warp,
        iPalette
    );

    // ── Layer 2: Foreground stars — few, bright, fast, long streaks ──
    vec3 layer2 = starLayer(
        p, radius, angle, time * 1.3 + 41.7,
        layer2Lanes,
        1.0,                           // full speed
        layer2Streak,                  // long streaks with overdrive headroom
        0.004,                         // bright
        warp,
        iPalette
    );

    color += layer0 + layer1 + layer2;

    // ── Central focal glow — the hyperspace tunnel origin ──
    float centerIntensity = exp(-radius * 12.0) * 0.15;
    float centerPulse = 0.85 + 0.15 * sin(time * 4.0);
    vec3 centerCore, centerTint;
    streakColors(iPalette, 0.5, centerCore, centerTint);
    color += mix(centerCore, centerTint, 0.3) * centerIntensity * centerPulse;

    // ── Subtle radial rays from center — barely visible structure ──
    float rays = 0.5 + 0.5 * sin(angle * 12.0 + time * 0.5);
    float rayMask = exp(-radius * 6.0) * 0.02 * rays;
    color += vec3(rayMask * 0.6, rayMask * 0.6, rayMask * 0.8);

    // ── Tone mapping ──
    // Soft clamp to preserve bright star cores without harsh clipping
    color = color / (color + vec3(1.0)); // Reinhard
    color = pow(clamp(color, 0.0, 1.0), vec3(0.92));

    fragColor = vec4(color, 1.0);
}
