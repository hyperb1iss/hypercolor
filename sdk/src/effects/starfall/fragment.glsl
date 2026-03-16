#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;
uniform int iPalette;
uniform float iSpeed;
uniform float iDensity;
uniform float iTrails;
uniform float iSparkle;
uniform float iAngle;
uniform float iSize;
uniform int iTailMode;

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

vec2 hash22(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * vec3(0.1031, 0.1030, 0.0973));
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.xx + p3.yz) * p3.zx);
}

// ─── HSV helpers ─────────────────────────────────────────────────────

vec3 hsv2rgb(vec3 c) {
    vec4 K = vec4(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    vec3 p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
    return c.z * mix(K.xxx, clamp(p - K.xxx, 0.0, 1.0), c.y);
}

// ─── Palette ────────────────────────────────────────────────────────

struct StarPalette {
    vec3 headColor;
    vec3 trailColor;
    vec3 fadeColor;
};

StarPalette getPalette(int id) {
    // Heads are tinted (not pure white) for LED pop — keeps whiteness ratio low
    // 0: Celestial — icy-blue head, cyan trail, deep blue fade
    if (id == 0) return StarPalette(
        vec3(0.85, 0.95, 1.00),
        vec3(0.00, 0.90, 1.00),
        vec3(0.00, 0.10, 0.27)
    );
    // 1: Aurora Rain — green-white head, green trail, purple fade
    if (id == 1) return StarPalette(
        vec3(0.85, 1.00, 0.90),
        vec3(0.31, 0.98, 0.48),
        vec3(0.49, 0.30, 1.00)
    );
    // 2: Ember Fall — warm-white head, orange trail, dark red fade
    if (id == 2) return StarPalette(
        vec3(1.00, 0.92, 0.75),
        vec3(1.00, 0.40, 0.00),
        vec3(0.27, 0.00, 0.00)
    );
    // 3: Frozen Tears — blue-white head, ice blue trail, deep navy fade
    if (id == 3) return StarPalette(
        vec3(0.88, 0.94, 1.00),
        vec3(0.38, 0.80, 1.00),
        vec3(0.00, 0.04, 0.16)
    );
    // 4: Neon Rain — pink-white head, hot pink trail, electric purple fade
    if (id == 4) return StarPalette(
        vec3(1.00, 0.88, 0.95),
        vec3(1.00, 0.00, 0.67),
        vec3(0.40, 0.00, 1.00)
    );
    // 5: Cosmic — warm-gold head, gold trail, magenta fade
    return StarPalette(
        vec3(1.00, 0.95, 0.82),
        vec3(1.00, 0.65, 0.00),
        vec3(1.00, 0.00, 1.00)
    );
}

// ─── Background star field ──────────────────────────────────────────

float starField(vec2 uv) {
    vec2 cell = floor(uv * 80.0);
    float h = hash21(cell);

    // Only ~8% of cells have a star
    if (h > 0.08) return 0.0;

    vec2 center = (cell + hash22(cell * 1.73)) / 80.0;
    float dist = length(uv - center) * 80.0;
    float brightness = smoothstep(0.6, 0.0, dist);

    // Gentle twinkle
    float twinkle = 0.6 + 0.4 * sin(iTime * (1.0 + h * 3.0) + h * 62.83);
    return brightness * twinkle * h * 0.4;
}

// ─── Tail mode color ────────────────────────────────────────────────
// 0: Palette (original), 1: Rainbow, 2: Ghostly, 3: Electric

vec3 getTailColor(vec3 headCol, vec3 trailCol, vec3 fadeCol, float energy, float particleId, float time, int mode) {
    // Palette — original head→trail→fade blend
    if (mode == 0) {
        vec3 headMix = mix(trailCol, headCol, pow(energy, 2.0));
        return mix(fadeCol, headMix, energy);
    }
    // Rainbow — hue shifts along the trail length
    if (mode == 1) {
        float hueBase = hash11(particleId * 7.31) + time * 0.1;
        float hueShift = (1.0 - energy) * 0.8;
        vec3 rainbow = hsv2rgb(vec3(fract(hueBase + hueShift), 0.9, 1.0));
        vec3 bright = mix(rainbow, headCol, pow(energy, 3.0));
        return mix(vec3(0.0), bright, energy);
    }
    // Ghostly — desaturated white-blue fade with ethereal glow
    if (mode == 2) {
        vec3 ghost = vec3(0.7, 0.8, 1.0);
        vec3 core = mix(ghost * 0.3, headCol, pow(energy, 2.5));
        float flicker = 0.8 + 0.2 * sin(particleId * 43.7 + time * 8.0 + energy * 12.0);
        return core * energy * flicker;
    }
    // Electric — bright forked appearance with palette accent
    vec3 spark = mix(trailCol, vec3(1.0), pow(energy, 1.5));
    float crackle = 0.7 + 0.3 * sin(particleId * 97.3 + energy * 30.0 + time * 15.0);
    vec3 elec = mix(fadeCol, spark * crackle, energy);
    return elec;
}

// ─── Particle system ────────────────────────────────────────────────

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    float aspect = iResolution.x / iResolution.y;

    // ── Control mapping ──
    float speed = max(iSpeed, 0.2);
    float time = iTime * (0.15 + speed * 0.25);

    // Column count: 8 at density=0, 24 at density=100
    float densityNorm = clamp(iDensity * 0.01, 0.0, 1.0);
    float numColumns = mix(8.0, 24.0, densityNorm);

    // Trail length multiplier: 0.3 at trails=0, 1.8 at trails=100
    float trailNorm = clamp(iTrails * 0.01, 0.0, 1.0);
    float trailMult = mix(0.3, 1.8, trailNorm);

    // Sparkle intensity
    float sparkleNorm = clamp(iSparkle * 0.01, 0.0, 1.0);

    // Star size: 0.5x at size=0, 3.0x at size=100
    float sizeNorm = clamp(iSize * 0.01, 0.0, 1.0);
    float sizeMult = mix(0.5, 3.0, sizeNorm);

    // Fall angle: degrees from vertical (-60 to +60)
    float angleRad = radians(clamp(iAngle, -60.0, 60.0));
    vec2 fallDir = vec2(sin(angleRad), -cos(angleRad));
    vec2 perpDir = vec2(cos(angleRad), sin(angleRad));

    StarPalette pal = getPalette(iPalette);
    int tailMode = iTailMode;

    // ── Background ──
    vec3 color = vec3(0.005, 0.005, 0.015);
    color += pal.fadeColor * 0.02;

    // Faint static star field
    float stars = starField(uv);
    color += vec3(0.6, 0.7, 0.9) * stars;

    // ── Falling particles ──
    // Aspect-corrected coordinates for circular glow
    vec2 uvAspect = vec2(uv.x * aspect, uv.y);

    // Project viewport corners onto fall/perp axes to find true travel bounds.
    // Viewport in aspect-corrected space: (0,0), (A,0), (0,1), (A,1)
    float f0 = 0.0;
    float f1 = aspect * fallDir.x;
    float f2 = fallDir.y;
    float f3 = aspect * fallDir.x + fallDir.y;
    float fallMin = min(min(f0, f1), min(f2, f3));
    float fallMax = max(max(f0, f1), max(f2, f3));

    float p0 = 0.0;
    float p1 = aspect * perpDir.x;
    float p2 = perpDir.y;
    float p3 = aspect * perpDir.x + perpDir.y;
    float perpMin = min(min(p0, p1), min(p2, p3));
    float perpMax = max(max(p0, p1), max(p2, p3));

    float fallSpan = fallMax - fallMin;
    float perpSpan = perpMax - perpMin;
    float spawnMargin = 0.25 + trailMult * 0.15;

    // Check particles in nearby columns (current + neighbors)
    // Integer loop with constant upper bound for GLSL ES 300 compatibility
    for (int iCol = -1; iCol < 26; iCol++) {
        float col = float(iCol);
        if (col > numColumns + 1.0) break;
        // Each column spawns 2-3 particles at different phases
        for (int iLayer = 0; iLayer < 3; iLayer++) {
            float layer = float(iLayer);
            float particleId = col * 3.0 + layer + 0.5;

            // Per-particle randomness
            float h0 = hash11(particleId * 7.31);
            float h1 = hash11(particleId * 13.17);
            float h2 = hash11(particleId * 23.71);
            float h3 = hash11(particleId * 37.93);
            float h4 = hash11(particleId * 51.37);

            // Skip some particles at low density / third layer
            if (layer >= 2.0 && h0 < 0.4) continue;

            // Speed variation per particle: 0.6x to 1.5x
            float particleSpeed = 0.6 + h1 * 0.9;

            // Column position along the perpendicular axis (jittered within slot)
            float colPerp = perpMin + (col + 0.15 + h0 * 0.7) / numColumns * perpSpan;

            // Fall position: travel from fallMin-margin → fallMax+margin
            // fallMin is the "upstream" edge (where particles enter from),
            // fallMax is the "downstream" edge (where they exit).
            // For angle=0, fallDir=(0,-1): top of screen has fallProj=-1 (=fallMin),
            // bottom has fallProj=0 (=fallMax). Particles travel min→max = top→bottom.
            float totalTravel = fallSpan + spawnMargin * 2.0;
            float period = totalTravel / (particleSpeed * 0.5 + 0.3);
            float phase = h2 * period;
            float t = mod(time * particleSpeed + phase, period);
            float progress = t / period;  // 0→1 over the travel

            float fallPos = fallMin - spawnMargin + progress * totalTravel;

            // Reconstruct screen position from fall/perp coordinates
            vec2 particlePos = fallPos * fallDir + colPerp * perpDir;

            // Skip particles fully off-screen (with margin for glow/trail)
            float margin = 0.3 * sizeMult;
            float pxNorm = particlePos.x / aspect;
            float py = particlePos.y;
            if (py < -margin || py > 1.0 + margin) continue;
            if (pxNorm < -margin || pxNorm > 1.0 + margin) continue;

            // ── Distance field ──
            vec2 delta = uvAspect - particlePos;

            // Project delta onto fall direction for trail orientation.
            // fallDir is already unit-length; particlePos is in aspect-corrected
            // space, and so is uvAspect, so the projection is direct.
            float alongFall = dot(delta, fallDir);
            float alongTrail = -alongFall;  // trail extends opposite to motion
            float perpTrail = length(delta - fallDir * alongFall);

            float ahead = max(0.0, -alongTrail);   // ahead of particle (short cutoff)
            float behind = max(0.0, alongTrail);    // behind particle (trail)

            // Horizontal squeeze for narrow trail (scaled by star size)
            float hSqueeze = (4.0 + h3 * 2.0) / sizeMult;

            // Trail length from controls + per-particle variation
            float trailLen = (0.15 + h3 * 0.25) * trailMult;
            float trailDecay = 1.0 / max(trailLen, 0.01);

            // Asymmetric distance: sharp above, stretched below
            float trailShape = length(vec2(
                perpTrail * hSqueeze,
                ahead * 8.0 / sizeMult + behind * (0.2 + 0.15 * (1.0 - trailNorm))
            ));

            // Size/brightness variation: some are foreground (bigger), some background (dimmer)
            float sizeFactor = (0.4 + h4 * 0.6) * sizeMult;
            float headBrightness = sizeFactor * (0.8 + h2 * 0.4);

            // ── Glow calculation ──
            float glowRadius = 0.0012 * sizeMult;
            float glowFloor = 0.0008 * sizeMult;
            float glow = headBrightness * glowRadius / (trailShape * trailShape + glowFloor);

            // Clamp to avoid firefly-level blowout at exact center
            glow = min(glow, 3.0);

            // ── Trail color ──
            float energy = exp(-behind * trailDecay);
            energy = pow(energy, 1.8);  // non-linear falloff for premium decay

            vec3 trailColor = getTailColor(
                pal.headColor, pal.trailColor, pal.fadeColor,
                energy, particleId, iTime, tailMode
            );

            // ── Sparkle: random trail pixel brightening ──
            if (sparkleNorm > 0.0 && behind > 0.01 && behind < trailLen * 0.9) {
                float sparkleHash = hash21(vec2(
                    floor(gl_FragCoord.x * 0.5 + particleId * 100.0),
                    floor(gl_FragCoord.y * 0.5 + time * 3.0)
                ));
                float sparkleThreshold = 1.0 - sparkleNorm * 0.15;
                if (sparkleHash > sparkleThreshold) {
                    float sparkleBoost = (sparkleHash - sparkleThreshold) / (1.0 - sparkleThreshold);
                    glow += sparkleBoost * 0.4 * energy * sizeFactor;
                    trailColor = mix(trailColor, pal.headColor, sparkleBoost * 0.5);
                }
            }

            // ── Head hotspot — extra bright single-pixel punch ──
            float headScale = sizeMult;
            float headDist = length(vec2(perpTrail * hSqueeze * 0.5, alongTrail * 6.0 / headScale));
            float headSpot = headBrightness * 0.0003 * headScale / (headDist * headDist + 0.0001 * headScale);
            headSpot = min(headSpot, 4.0);

            // Accumulate
            color += trailColor * glow;
            color += pal.headColor * headSpot;
        }
    }

    // ── Tone mapping ──
    // Soft clamp to preserve trail gradients while allowing hot heads
    color = color / (color + vec3(0.8));
    color = pow(clamp(color, 0.0, 1.0), vec3(0.92));

    fragColor = vec4(color, 1.0);
}
