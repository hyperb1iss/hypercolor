#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

uniform float iSpeed;
uniform float iScale;
uniform float iEdgeGlow;
uniform float iGrowth;
uniform int iPalette;
uniform int iScene;

// ── Hash functions ─────────────────────────────────────────────────

float hash21(vec2 p) {
    p = fract(p * vec2(443.897, 441.423));
    p += dot(p, p + 19.19);
    return fract(p.x * p.y);
}

vec2 hash22(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * vec3(0.1031, 0.1030, 0.0973));
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.xx + p3.yz) * p3.zy);
}

// ── LED-safe palettes ──────────────────────────────────────────────
// Tier 1/2 hues, saturation ≥ 85 %, whiteness ratio < 0.25.
// Every entry has at least one channel near zero.

vec3 ledPrimary(int pal) {
    if (pal == 0) return vec3(0.88, 0.21, 1.00); // SilkCircuit – electric purple
    if (pal == 1) return vec3(0.08, 0.42, 1.00); // Ice         – sapphire
    if (pal == 2) return vec3(0.00, 0.62, 1.00); // Frost       – azure
    if (pal == 3) return vec3(0.04, 1.00, 0.38); // Aurora      – vivid green
    return vec3(1.00, 0.05, 0.78);               // Cyberpunk   – hot magenta
}

vec3 ledSecondary(int pal) {
    if (pal == 0) return vec3(0.00, 1.00, 0.88); // neon cyan
    if (pal == 1) return vec3(0.00, 0.88, 1.00); // bright cyan
    if (pal == 2) return vec3(0.00, 0.90, 0.78); // teal
    if (pal == 3) return vec3(0.00, 0.84, 0.72); // teal
    return vec3(0.04, 0.18, 1.00);               // deep blue
}

vec3 ledAccent(int pal) {
    if (pal == 0) return vec3(1.00, 0.42, 0.76); // coral
    if (pal == 1) return vec3(0.32, 0.06, 0.96); // deep indigo
    if (pal == 2) return vec3(0.06, 0.30, 1.00); // cobalt
    if (pal == 3) return vec3(0.58, 0.14, 1.00); // violet
    return vec3(0.00, 0.92, 1.00);               // electric cyan
}

vec3 paletteAt(float t, int pal) {
    t = fract(t) * 3.0;
    float f = fract(t);
    vec3 a, b;
    if (t < 1.0)      { a = ledPrimary(pal);   b = ledSecondary(pal); }
    else if (t < 2.0)  { a = ledSecondary(pal); b = ledAccent(pal); }
    else               { a = ledAccent(pal);    b = ledPrimary(pal); }
    return sqrt(mix(a * a, b * b, f));
}

// ── Voronoi ────────────────────────────────────────────────────────
// Returns: (nearestDist, edgeDist, cellSeed, unused)
// Uses animated cell points for organic crystal drift.

vec4 voronoi(vec2 p, float time) {
    vec2 ip = floor(p);
    vec2 fp = fract(p);

    float d1 = 10.0, d2 = 10.0;
    float seed1 = 0.0;

    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            vec2 n      = vec2(float(x), float(y));
            vec2 cellId = ip + n;
            vec2 pt     = hash22(cellId);
            // Gentle drift — points oscillate within their cell
            pt = 0.5 + 0.38 * sin(time * 0.35 + 6.28318 * pt);
            float d = length(n + pt - fp);

            if (d < d1) {
                d2    = d1;
                d1    = d;
                seed1 = hash21(cellId);
            } else if (d < d2) {
                d2 = d;
            }
        }
    }

    return vec4(d1, d2 - d1, seed1, 0.0);
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    vec2 p  = uv * 2.0 - 1.0;
    p.x *= iResolution.x / iResolution.y;

    float speed = max(iSpeed, 0.1);
    float t     = iTime * (0.30 + speed * 0.90);

    float scaleMix = clamp(iScale / 100.0, 0.0, 1.0);
    float growth   = clamp(iGrowth / 100.0, 0.0, 1.0);
    float glow     = clamp(iEdgeGlow / 100.0, 0.0, 1.0);

    // Cell density — larger cells at low scale for LED readability
    float density = mix(1.5, 4.5, scaleMix);

    int sc = clamp(iScene, 0, 3);

    // ── Scene coordinate transforms ────────────────────────────────
    vec2 vp = p;

    if (sc == 0) {
        // Lattice: gentle slow rotation
        float theta = sin(t * 0.06) * 0.15;
        float ct = cos(theta), st = sin(theta);
        vp = mat2(ct, -st, st, ct) * vp;
        vp += vec2(sin(t * 0.12), cos(t * 0.09)) * 0.05;
    } else if (sc == 1) {
        // Shardfield: stretched coordinates → elongated crystal shards
        float theta = t * 0.04;
        float ct = cos(theta), st = sin(theta);
        vp = mat2(ct, -st, st, ct) * vp;
        vp.x *= 1.0 + 0.35 * sin(t * 0.08);
    } else if (sc == 2) {
        // Prism: slow drift (dual-layer Voronoi handled below)
        vp += vec2(cos(t * 0.14), sin(t * 0.11)) * 0.08;
    } else {
        // Signal: slight drift
        vp += vec2(sin(t * 0.10), cos(t * 0.13)) * 0.04;
    }

    // ── Primary Voronoi layer ──────────────────────────────────────
    vec4 vor      = voronoi(vp * density, t);
    float nearDist = vor.x;
    float edgeDist = vor.y;
    float cellSeed = vor.z;

    // ── Growth wave — scene-dependent modulation ───────────────────
    float growthWave = 1.0;
    float radial     = length(p);

    if (sc == 3) {
        // Signal: traveling brightness wave (radial outward)
        float wave = sin(radial * 4.0 - t * 2.0);
        growthWave = 0.35 + 0.65 * smoothstep(-0.3, 0.5, wave);
    }

    // ── Cell fill ──────────────────────────────────────────────────
    // Growth controls how much of each cell is lit:
    //   low  → small bright dots at cell centers (sparse, dark)
    //   high → cells nearly fully filled (thin dark boundaries)
    float growthRadius = mix(0.12, 0.48, growth);
    float fill = smoothstep(growthRadius, growthRadius * 0.25, nearDist);
    fill *= growthWave;

    // Per-cell brightness variation — some facets brighter
    float cellBrightness = 0.5 + 0.5 * cellSeed;
    fill *= cellBrightness;

    // ── Edge glow ──────────────────────────────────────────────────
    // Broad glow at cell boundaries — NOT thin lines.
    float edgeWidth = mix(0.04, 0.16, glow);
    float edge      = smoothstep(edgeWidth, 0.0, edgeDist) * glow;

    // ── Prism scene: second Voronoi layer ──────────────────────────
    float secondFill = 0.0;
    float secondSeed = 0.0;
    if (sc == 2) {
        vec4 vor2 = voronoi(vp * density * 0.55 + vec2(3.7, -2.1), t * 0.7);
        float fill2 = smoothstep(growthRadius * 1.2, growthRadius * 0.35, vor2.x);
        fill2 *= 0.5 + 0.5 * vor2.z;
        secondFill = fill2 * 0.45;
        secondSeed = vor2.z;
    }

    // ── Color composition ──────────────────────────────────────────
    vec3 cellColor = paletteAt(cellSeed + t * 0.02, iPalette);
    vec3 edgeColor = ledAccent(iPalette);

    vec3 col = vec3(0.0);

    // Cell fill (screen blend)
    vec3 fillContrib = cellColor * fill;
    col = col + fillContrib * (1.0 - col);

    // Prism second layer (screen blend)
    if (sc == 2) {
        vec3 layer2Color   = paletteAt(secondSeed + 0.33 + t * 0.025, iPalette);
        vec3 layer2Contrib = layer2Color * secondFill;
        col = col + layer2Contrib * (1.0 - col);
    }

    // Edge glow (screen blend)
    vec3 edgeContrib = edgeColor * edge;
    col = col + edgeContrib * (1.0 - col);

    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
