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
// cellLocalVec: pixel position relative to nearest cell center.

vec4 voronoi(vec2 p, float time, out vec2 cellLocalVec) {
    vec2 ip = floor(p);
    vec2 fp = fract(p);

    float d1 = 10.0, d2 = 10.0;
    float seed1 = 0.0;
    vec2 nearestDiff = vec2(0.0);

    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            vec2 n      = vec2(float(x), float(y));
            vec2 cellId = ip + n;
            vec2 pt     = hash22(cellId);
            pt = 0.5 + 0.38 * sin(time * 0.35 + 6.28318 * pt);
            vec2 diff = n + pt - fp;
            float d = length(diff);

            if (d < d1) {
                d2    = d1;
                d1    = d;
                seed1 = hash21(cellId);
                nearestDiff = diff;
            } else if (d < d2) {
                d2 = d;
            }
        }
    }

    cellLocalVec = -nearestDiff;
    return vec4(d1, d2 - d1, seed1, 0.0);
}

// Convenience overload when cell-local vector isn't needed
vec4 voronoi(vec2 p, float time) {
    vec2 unused;
    return voronoi(p, time, unused);
}

// ── Dendrite SDF ───────────────────────────────────────────────────
// Distance to 6-fold symmetric branching arms (depth 1 sub-branches).

float segDist(vec2 p, vec2 dir, float len) {
    float proj = clamp(dot(p, dir), 0.0, len);
    return length(p - dir * proj);
}

float dendriteDist(vec2 local, float seed, float time) {
    float d = 10.0;
    float armLen = 0.38;
    float breathe = 0.82 + 0.18 * sin(time * 0.8 + seed * 6.28);
    armLen *= breathe;

    for (int i = 0; i < 6; i++) {
        float a = float(i) * 1.0472 + seed * 0.2;
        vec2 dir = vec2(cos(a), sin(a));

        // Main arm
        d = min(d, segDist(local, dir, armLen));

        // Sub-branch origin at 55% along arm
        vec2 bp = dir * armLen * 0.55;
        float subLen = armLen * 0.5;

        // Left sub-branch (+60°)
        vec2 subDir = vec2(cos(a + 1.0472), sin(a + 1.0472));
        d = min(d, segDist(local - bp, subDir, subLen));

        // Right sub-branch (-60°)
        subDir = vec2(cos(a - 1.0472), sin(a - 1.0472));
        d = min(d, segDist(local - bp, subDir, subLen));
    }

    return d;
}

// ── Interference field ─────────────────────────────────────────────
// Superposition of coherent waves from neighboring Voronoi cell centers.

float interferenceField(vec2 p, vec2 voronoiCoord, float density, float time, float glow) {
    vec2 ip = floor(voronoiCoord);
    float wave = 0.0;
    float freq = 6.0 + glow * 4.0;

    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            vec2 cellId = ip + vec2(float(x), float(y));
            vec2 pt = hash22(cellId);
            pt = 0.5 + 0.38 * sin(time * 0.35 + 6.28318 * pt);
            vec2 center = (cellId + pt) / density;
            float dist = length(p - center);
            float angle = atan(p.y - center.y, p.x - center.x);
            // 6-fold angular modulation → hexagonal wave fronts
            float hexMod = 1.0 + (0.1 + glow * 0.1) * cos(angle * 6.0 + hash21(cellId) * 6.28 + time * 0.3);
            wave += sin(dist * freq * hexMod - time * 1.2 + hash21(cellId) * 6.28);
        }
    }

    return wave / 9.0;
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

    int sc = clamp(iScene, 0, 6);

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
    } else if (sc == 3) {
        // Signal: slight drift
        vp += vec2(sin(t * 0.10), cos(t * 0.13)) * 0.04;
    } else if (sc == 4) {
        // Dendrite: gentle rotation + slow drift
        float theta = sin(t * 0.05) * 0.12;
        float ct = cos(theta), st = sin(theta);
        vp = mat2(ct, -st, st, ct) * vp;
        vp += vec2(sin(t * 0.08), cos(t * 0.06)) * 0.04;
    } else if (sc == 5) {
        // Koch: slow spin
        float theta = t * 0.03;
        float ct = cos(theta), st = sin(theta);
        vp = mat2(ct, -st, st, ct) * vp;
        vp += vec2(cos(t * 0.1), sin(t * 0.08)) * 0.03;
    } else {
        // Interference: gentle drift
        vp += vec2(cos(t * 0.12), sin(t * 0.09)) * 0.06;
    }

    // ── Primary Voronoi layer ──────────────────────────────────────
    vec2 cellLocal;
    vec4 vor      = voronoi(vp * density, t, cellLocal);
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
    } else if (sc == 6) {
        // Interference: radial breathing
        float wave = sin(radial * 3.0 - t * 1.5);
        growthWave = 0.4 + 0.6 * smoothstep(-0.2, 0.4, wave);
    }

    // ── Cell fill ──────────────────────────────────────────────────
    float growthRadius = mix(0.12, 0.48, growth);
    float fill = smoothstep(growthRadius, growthRadius * 0.25, nearDist);
    fill *= growthWave;

    float cellBrightness = 0.5 + 0.5 * cellSeed;
    fill *= cellBrightness;

    // ── Edge glow ──────────────────────────────────────────────────
    float edgeWidth = mix(0.04, 0.16, glow);
    float edge      = smoothstep(edgeWidth, 0.0, edgeDist) * glow;

    // ── Dendrite: branch distance field replaces cell fill ────────
    if (sc == 4) {
        float dd = dendriteDist(cellLocal, cellSeed, t);
        float branchWidth = mix(0.04, 0.1, glow);
        fill = smoothstep(branchWidth, branchWidth * 0.2, dd);
        fill *= cellBrightness * growthWave;
    }

    // ── Koch: multi-octave fractal Voronoi edges ─────────────────
    if (sc == 5) {
        // Layer 2: 3x finer Voronoi edges
        vec4 vor2 = voronoi(vp * density * 3.0, t * 0.8);
        float edge2 = smoothstep(edgeWidth * 0.7, 0.0, vor2.y) * glow * 0.5;
        edge = max(edge, edge2);
        // Layer 3: 9x finer (subtle fractal detail)
        vec4 vor3 = voronoi(vp * density * 9.0, t * 0.6);
        float edge3 = smoothstep(edgeWidth * 0.5, 0.0, vor3.y) * glow * 0.25;
        edge = max(edge, edge3);
        // Reduce fill so fractal edges dominate
        fill *= 0.5;
    }

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

    // ── Interference: wave superposition field ────────────────────
    float intfField = 0.0;
    if (sc == 6) {
        intfField = interferenceField(p, vp * density, density, t, glow);
    }

    // ── Color composition ──────────────────────────────────────────
    vec3 cellColor = paletteAt(cellSeed + t * 0.02, iPalette);
    vec3 edgeColor = ledAccent(iPalette);

    vec3 col = vec3(0.0);

    if (sc == 6) {
        // Interference: color by wave amplitude
        float amp = intfField * 0.5 + 0.5;
        amp *= amp; // gamma for LED contrast
        vec3 waveColor = paletteAt(amp + t * 0.01, iPalette);
        col = waveColor * amp * growthWave * (0.5 + glow * 0.5);
        // Add cell edge structure underneath
        vec3 edgeContrib = edgeColor * edge * 0.4;
        col = col + edgeContrib * (1.0 - col);
    } else {
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
    }

    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
