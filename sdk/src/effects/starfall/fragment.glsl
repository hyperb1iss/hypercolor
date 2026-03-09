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

    StarPalette pal = getPalette(iPalette);

    // ── Background ──
    vec3 color = vec3(0.005, 0.005, 0.015);
    color += pal.fadeColor * 0.02;

    // Faint static star field
    float stars = starField(uv);
    color += vec3(0.6, 0.7, 0.9) * stars;

    // ── Falling particles ──
    // Aspect-corrected coordinates for circular glow
    vec2 uvAspect = vec2(uv.x * aspect, uv.y);

    float cellWidth = aspect / numColumns;

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

            // Horizontal position: jittered within column
            float px = (col + 0.15 + h0 * 0.7) * cellWidth;
            float pxNorm = px / aspect;

            // Vertical position: falling with wrap
            float period = 1.0 / (particleSpeed * 0.5 + 0.3);
            float phase = h2 * period;
            float py = 1.0 - fract((time * particleSpeed + phase) / period);

            // Particle world position (aspect-corrected)
            vec2 particlePos = vec2(px, py);

            // ── Distance field ──
            vec2 delta = uvAspect - particlePos;

            // Asymmetric: short above particle, long trail below
            float ahead = max(0.0, -delta.y);    // pixels above (short cutoff)
            float behind = max(0.0, delta.y);     // pixels below (trail)

            // Horizontal squeeze for narrow trail
            float hSqueeze = 4.0 + h3 * 2.0;

            // Trail length from controls + per-particle variation
            float trailLen = (0.15 + h3 * 0.25) * trailMult;
            float trailDecay = 1.0 / max(trailLen, 0.01);

            // Asymmetric distance: sharp above, stretched below
            float trailShape = length(vec2(
                delta.x * hSqueeze,
                ahead * 8.0 + behind * (0.2 + 0.15 * (1.0 - trailNorm))
            ));

            // Size/brightness variation: some are foreground (bigger), some background (dimmer)
            float sizeFactor = 0.4 + h4 * 0.6;
            float headBrightness = sizeFactor * (0.8 + h2 * 0.4);

            // ── Glow calculation ──
            float glow = headBrightness * 0.0012 / (trailShape * trailShape + 0.0008);

            // Clamp to avoid firefly-level blowout at exact center
            glow = min(glow, 3.0);

            // ── Trail color gradient ──
            float energy = exp(-behind * trailDecay);
            energy = pow(energy, 1.8);  // non-linear falloff for premium decay

            // Head → trail → fade color blend
            vec3 headMix = mix(pal.trailColor, pal.headColor, pow(energy, 2.0));
            vec3 trailColor = mix(pal.fadeColor, headMix, energy);

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
            float headDist = length(delta * vec2(hSqueeze * 0.5, 6.0));
            float headSpot = headBrightness * 0.0003 / (headDist * headDist + 0.0001);
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
