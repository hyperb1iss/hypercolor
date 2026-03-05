#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2 iResolution;

// Controls
uniform float iSpeed;
uniform float iDensity;
uniform float iTrailLength;
uniform float iCharSize;
uniform int iPalette;

// ── Hash ───────────────────────────────────────────────────────────────

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

float hash11(float p) {
    p = fract(p * 0.1031);
    p *= p + 33.33;
    p *= p + p;
    return fract(p);
}

// ── Palettes ───────────────────────────────────────────────────────────

vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

vec3 paletteColor(float t, int id) {
    // Matrix green (default)
    if (id == 0) return mix(vec3(0.0, 0.15, 0.0), vec3(0.2, 1.0, 0.3), t);
    // Phosphor amber
    if (id == 1) return mix(vec3(0.15, 0.08, 0.0), vec3(1.0, 0.7, 0.1), t);
    // SilkCircuit
    if (id == 2) return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
    // Cyberpunk
    if (id == 3) return iqPalette(t, vec3(0.5, 0.2, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 1.0), vec3(0.8, 0.1, 0.6));
    // Ice
    if (id == 4) return mix(vec3(0.0, 0.05, 0.15), vec3(0.4, 0.8, 1.0), t);
    return mix(vec3(0.0, 0.15, 0.0), vec3(0.2, 1.0, 0.3), t);
}

// ── Character simulation ──────────────────────────────────────────────

// Pseudo-glyph: creates a character-like pattern in a cell
float glyphCell(vec2 uv, float seed) {
    // 5x7 grid within the cell
    vec2 grid = floor(uv * vec2(5.0, 7.0));
    float cellHash = hash21(grid + seed * 100.0);

    // Create character-like patterns with ~40% fill
    float fill = step(0.6, cellHash);

    // Add some structural patterns (vertical/horizontal bars)
    float vertBar = step(0.7, hash11(grid.x + seed * 50.0));
    float horizBar = step(0.8, hash11(grid.y + seed * 30.0));

    return max(fill, max(vertBar * 0.5, horizBar * 0.3));
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution;
    float time = iTime * iSpeed * 0.4;

    // Character grid
    float charScale = 8.0 + iCharSize * 0.4;
    float columns = floor(iResolution.x / charScale);
    float rows = floor(iResolution.y / charScale);

    vec2 cellUV = uv * vec2(columns, rows);
    vec2 cell = floor(cellUV);
    vec2 local = fract(cellUV);

    // Per-column properties
    float colSeed = hash11(cell.x * 0.7);
    float colSpeed = 0.5 + colSeed * 1.5;
    float colOffset = colSeed * 100.0;

    // Column active probability (density control)
    float colActive = step(1.0 - iDensity * 0.01, hash11(cell.x * 1.3 + 7.0));

    // Rain drop position (scrolling down)
    float dropPos = fract(time * colSpeed * 0.15 + colOffset);
    float dropY = dropPos * (rows + iTrailLength * 0.3);

    // Distance from this cell to the drop head
    float cellY = rows - cell.y; // flip so rain falls down
    float distFromHead = cellY - dropY;

    // Trail: bright at head, fading behind
    float trailLen = 5.0 + iTrailLength * 0.25;
    float inTrail = smoothstep(trailLen, 0.0, distFromHead) * step(0.0, distFromHead);

    // Head glow (brightest point)
    float headGlow = exp(-distFromHead * distFromHead * 2.0) * step(-0.5, distFromHead);

    // Character changes periodically
    float charSeed = hash21(cell) + floor(time * 2.0 + cell.y * 0.1) * 0.1;
    float glyph = glyphCell(local, charSeed);

    // ── Compose ────────────────────────────────────────────────────────

    // Character visibility
    float charBrightness = glyph * inTrail * colActive;

    // Head character is always bright and full
    charBrightness = max(charBrightness, headGlow * colActive * 0.8);

    // Color: head is white/bright, trail fades to palette color
    float headAmount = headGlow / (headGlow + 0.1);
    vec3 trailColor = paletteColor(inTrail * 0.8, iPalette);
    vec3 headColor = paletteColor(1.0, iPalette) * 1.5 + vec3(0.5);

    vec3 col = mix(trailColor, headColor, headAmount) * charBrightness;

    // Multiple rain streams per column (depth layers)
    for (int layer = 1; layer < 3; layer++) {
        float fl = float(layer);
        float layerSeed = hash11(cell.x * 0.7 + fl * 37.0);
        float layerSpeed = 0.3 + layerSeed * 1.0;
        float layerOffset = layerSeed * 100.0 + fl * 33.0;
        float layerActive = step(1.0 - iDensity * 0.006, hash11(cell.x * 1.3 + fl * 17.0));

        float lDropPos = fract(time * layerSpeed * 0.15 + layerOffset);
        float lDropY = lDropPos * (rows + iTrailLength * 0.2);
        float lDist = cellY - lDropY;

        float lTrail = smoothstep(trailLen * 0.7, 0.0, lDist) * step(0.0, lDist);
        float lGlyph = glyphCell(local, hash21(cell + fl * 50.0) + floor(time * 1.5) * 0.1);

        // Dimmer background layers
        float dimming = 0.3 - fl * 0.08;
        col += paletteColor(lTrail * 0.6, iPalette) * lGlyph * lTrail * layerActive * dimming;
    }

    // Subtle background glow in active columns
    float bgGlow = colActive * 0.015 * paletteColor(0.1, iPalette).g;
    col += vec3(0.0, bgGlow, 0.0);

    // Depth-of-field: slight blur on background layers
    col = col / (1.0 + col * 0.3);

    fragColor = vec4(clamp(col, 0.0, 1.0), 1.0);
}
