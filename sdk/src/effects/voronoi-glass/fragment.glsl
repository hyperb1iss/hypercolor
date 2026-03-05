#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

// Controls
uniform float iSpeed;
uniform float iScale;
uniform float iEdgeGlow;
uniform float iColorShift;
uniform int iPalette;
uniform int iDistanceMode;

// ── Noise ──────────────────────────────────────────────────────────────

vec2 hash22(vec2 p) {
    p = fract(p * vec2(443.8975, 397.2973));
    p += dot(p, p.yx + 19.19);
    return fract(vec2(p.x * p.y, p.y * p.x));
}

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// ── Palettes ───────────────────────────────────────────────────────────

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    if (id == 0) return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
    if (id == 1) return iqPalette(t, vec3(0.2, 0.5, 0.4), vec3(0.3, 0.4, 0.4), vec3(0.8, 0.7, 0.9), vec3(0.6, 0.3, 0.7));
    if (id == 2) return iqPalette(t, vec3(0.5, 0.2, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 1.0), vec3(0.8, 0.1, 0.6));
    if (id == 3) return iqPalette(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 1.0), vec3(0.0, 0.1, 0.2));
    if (id == 4) return iqPalette(t, vec3(0.5, 0.2, 0.0), vec3(0.5, 0.4, 0.2), vec3(1.0, 0.7, 0.4), vec3(0.0, 0.15, 0.2));
    if (id == 5) return iqPalette(t, vec3(0.6, 0.4, 0.7), vec3(0.3, 0.3, 0.3), vec3(0.6, 0.8, 1.0), vec3(0.7, 0.3, 0.6));
    return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
}

// ── Voronoi ────────────────────────────────────────────────────────────

// Returns (F1, F2, cell_id_hash) for stained glass effect
vec3 voronoi(vec2 p, float time) {
    vec2 i = floor(p);
    vec2 f = fract(p);

    float F1 = 1.0;
    float F2 = 1.0;
    float cellId = 0.0;
    vec2 nearestPoint = vec2(0.0);

    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            vec2 neighbor = vec2(float(x), float(y));
            vec2 cell = i + neighbor;
            vec2 point = hash22(cell);

            // Animate points — slow organic drift
            point = 0.5 + 0.45 * sin(time * 0.4 + point * 6.28318);

            vec2 diff = neighbor + point - f;

            // Distance modes
            float d;
            if (iDistanceMode == 0) {
                d = dot(diff, diff); // Euclidean²
            } else if (iDistanceMode == 1) {
                d = abs(diff.x) + abs(diff.y); // Manhattan
            } else {
                d = max(abs(diff.x), abs(diff.y)); // Chebyshev
            }

            if (d < F1) {
                F2 = F1;
                F1 = d;
                nearestPoint = cell;
                cellId = hash21(cell);
            } else if (d < F2) {
                F2 = d;
            }
        }
    }

    return vec3(sqrt(F1), sqrt(F2), cellId);
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    vec2 aspect = vec2(iResolution.x / iResolution.y, 1.0);
    float time = iTime * iSpeed * 0.2;

    // Scale and center
    float scale = 3.0 + iScale * 0.12;
    vec2 p = (uv - 0.5) * aspect * scale;

    // Slow global rotation for dynamism
    float angle = time * 0.05;
    mat2 rot = mat2(cos(angle), -sin(angle), sin(angle), cos(angle));
    p = rot * p;

    // Compute Voronoi
    vec3 vor = voronoi(p, time);
    float F1 = vor.x;
    float F2 = vor.y;
    float cellId = vor.z;

    // ── Cell fill color ────────────────────────────────────────────────
    float colorT = cellId + time * iColorShift * 0.003;
    vec3 cellColor = paletteColor(colorT, iPalette);

    // Brightness varies per cell — stained glass panes
    float cellBrightness = 0.3 + 0.4 * cellId;

    // Subtle inner gradient (darker toward edges of each cell)
    float innerGradient = smoothstep(0.0, 0.5, F1);
    cellBrightness *= 0.7 + 0.3 * (1.0 - innerGradient);

    vec3 col = cellColor * cellBrightness;

    // ── Edge glow (F2 - F1 technique) ──────────────────────────────────
    float edgeDist = F2 - F1;
    float edgeWidth = 0.02 + iEdgeGlow * 0.002;

    // Crisp edge line
    float edge = smoothstep(edgeWidth, edgeWidth * 0.3, edgeDist);

    // Soft glow around edges
    float edgeGlow = exp(-edgeDist * (8.0 + iEdgeGlow * 0.3)) * iEdgeGlow * 0.012;

    // Edge color: brighter version of surrounding cells
    vec3 edgeColor = paletteColor(colorT + 0.3, iPalette);

    // Combine: cell fill + edge highlight
    col = mix(col, edgeColor * 1.5, edge * 0.8);
    col += edgeColor * edgeGlow;

    // ── Light transmission effect ──────────────────────────────────────
    // Simulate light passing through colored glass
    float lightAngle = sin(time * 0.15) * 0.5;
    float lightDir = dot(normalize(vec2(cos(lightAngle), sin(lightAngle))), uv - 0.5);
    float lightIntensity = 0.8 + 0.2 * lightDir;
    col *= lightIntensity;

    // ── Subtle caustics ────────────────────────────────────────────────
    vec3 vor2 = voronoi(p * 2.0 + vec2(time * 0.1), time * 1.3);
    float caustic = smoothstep(0.4, 0.0, vor2.x) * 0.08;
    col += caustic * edgeColor;

    // Tonemapping
    col = col / (1.0 + col * 0.4);
    col = pow(col, vec3(0.95));

    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
