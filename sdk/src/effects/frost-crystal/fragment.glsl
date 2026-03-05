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

float hash21(vec2 p) {
    p = fract(p * vec2(443.897, 441.423));
    p += dot(p, p + 19.19);
    return fract(p.x * p.y);
}

float lineMask(float d, float width) {
    float aa = max(fwidth(d) * 0.9, 0.0008);
    return 1.0 - smoothstep(width, width + aa, d);
}

vec3 latticeCoords(vec2 p, float density) {
    float u = dot(p, vec2(1.0, 0.0)) * density;
    float v = dot(p, vec2(0.5, 0.8660254)) * density;
    float w = dot(p, vec2(-0.5, 0.8660254)) * density;
    return vec3(u, v, w);
}

vec3 paletteColor(int palette, int slot) {
    if (palette == 0) {
        if (slot == 0) return vec3(0.02, 0.01, 0.07);
        if (slot == 1) return vec3(0.06, 0.02, 0.10);
        if (slot == 2) return vec3(0.88, 0.20, 1.00);
        if (slot == 3) return vec3(0.36, 0.80, 0.76);
        return vec3(0.84, 0.72, 0.28);
    }

    if (palette == 1) {
        if (slot == 0) return vec3(0.01, 0.04, 0.08);
        if (slot == 1) return vec3(0.03, 0.10, 0.15);
        if (slot == 2) return vec3(0.22, 0.78, 1.00);
        if (slot == 3) return vec3(0.38, 0.82, 0.88);
        return vec3(0.60, 0.88, 0.95);
    }

    if (palette == 2) {
        if (slot == 0) return vec3(0.02, 0.05, 0.11);
        if (slot == 1) return vec3(0.05, 0.14, 0.20);
        if (slot == 2) return vec3(0.33, 0.62, 1.00);
        if (slot == 3) return vec3(0.50, 0.74, 0.90);
        return vec3(0.65, 0.84, 0.98);
    }

    if (palette == 3) {
        if (slot == 0) return vec3(0.02, 0.03, 0.08);
        if (slot == 1) return vec3(0.07, 0.04, 0.14);
        if (slot == 2) return vec3(0.22, 1.00, 0.78);
        if (slot == 3) return vec3(0.56, 0.26, 0.82);
        return vec3(0.82, 0.56, 0.94);
    }

    if (slot == 0) return vec3(0.03, 0.01, 0.05);
    if (slot == 1) return vec3(0.10, 0.02, 0.10);
    if (slot == 2) return vec3(1.00, 0.22, 0.70);
    if (slot == 3) return vec3(0.16, 0.78, 0.58);
    return vec3(0.95, 0.72, 0.30);
}

vec4 sceneLattice(vec3 coords, vec3 axisDist, vec2 local, float width, float growth, float t) {
    float a = lineMask(axisDist.x, width);
    float b = lineMask(axisDist.y, width);
    float c = lineMask(axisDist.z, width);
    float grid = max(max(a, b), c);
    float node = clamp(a * b + b * c + c * a, 0.0, 1.0);

    float ringRadius = mix(0.16, 0.41, growth);
    float ring = lineMask(abs((abs(local.x) + abs(local.y)) - ringRadius), width * 1.12);
    float runner = lineMask(abs(fract((coords.x - coords.y) * 0.5 + t * 0.16) - 0.5), width * 0.9);

    float primary = max(grid, ring * 0.56);
    float accent = node * 0.82 + ring * 0.44;
    float highlight = node * 0.70 + runner * 0.36;
    float fill = 0.14 + 0.20 * node;

    return vec4(primary, accent, highlight, fill);
}

vec4 sceneShardfield(vec3 coords, vec3 axisDist, vec2 local, float width, float growth, float t) {
    float a = lineMask(axisDist.x, width);
    float b = lineMask(axisDist.y, width);
    float c = lineMask(axisDist.z, width);
    float grid = max(max(a, b), c);
    float node = clamp(a * b + b * c + c * a, 0.0, 1.0);

    vec2 id = floor(coords.xy);
    float seed = hash21(id);
    float phase = (seed - 0.5) * 0.24 + sin(t + seed * 6.28318) * 0.02;

    float slashA = lineMask(abs(local.x * 0.92 + local.y * 0.54 + phase), width * 0.72);
    float slashB = lineMask(abs(local.x * 0.48 - local.y * 0.95 - phase * 0.7), width * 0.72);
    float facetRadius = mix(0.14, 0.44, growth * 0.85 + seed * 0.15);
    float facet = lineMask(abs(max(abs(local.x), abs(local.y)) - facetRadius), width * 1.2);
    float seam = lineMask(abs(fract((coords.z + t * 0.22) * 0.65) - 0.5), width * 0.92);

    float primary = max(grid * 0.66, slashA * 0.95);
    float accent = max(facet * 0.72, slashB * 0.82);
    float highlight = seam * 0.42 + slashA * slashB * 1.05 + node * 0.45;
    float fill = 0.10 + 0.22 * facet;

    return vec4(primary, accent, highlight, fill);
}

vec4 scenePrism(vec3 coords, vec3 axisDist, vec2 local, float width, float growth, float t) {
    float a = lineMask(axisDist.x, width);
    float b = lineMask(axisDist.y, width);
    float c = lineMask(axisDist.z, width);
    float grid = max(max(a, b), c);
    float node = clamp(a * b + b * c + c * a, 0.0, 1.0);

    float hex = max(abs(local.x) * 0.8660254 + abs(local.y) * 0.5, abs(local.y));
    float outer = lineMask(abs(hex - mix(0.18, 0.45, growth)), width * 1.2);
    float inner = lineMask(abs(hex - mix(0.08, 0.30, growth)), width * 0.95);
    float spokes = max(lineMask(abs(local.x), width * 0.85), lineMask(abs(local.y), width * 0.85));
    float runner = lineMask(abs(fract((coords.x + coords.y + t * 0.25) * 0.5) - 0.5), width * 0.95);

    float primary = max(grid * 0.60, outer);
    float accent = max(inner * 0.80, spokes * 0.45);
    float highlight = node * 0.72 + runner * 0.30 + inner * 0.45;
    float fill = 0.09 + 0.25 * outer;

    return vec4(primary, accent, highlight, fill);
}

vec4 sceneSignal(vec3 coords, vec3 axisDist, vec2 local, float width, float growth, float t) {
    float a = lineMask(axisDist.x, width);
    float b = lineMask(axisDist.y, width);
    float c = lineMask(axisDist.z, width);
    float grid = max(max(a, b), c);
    float node = clamp(a * b + b * c + c * a, 0.0, 1.0);

    float offset = mix(0.05, 0.25, growth);
    float chevronA = lineMask(abs(abs(local.x) - local.y - offset), width);
    float chevronB = lineMask(abs(abs(local.x) + local.y - offset), width);
    float bars = lineMask(abs(fract(coords.z * 0.5 + t * 0.24) - 0.5), width * 0.95);
    float rail = lineMask(abs(local.y), width * 0.8);

    float primary = max(grid * 0.55, max(chevronA, chevronB * 0.80));
    float accent = max(bars * 0.62, rail * 0.55);
    float highlight = node * 0.78 + max(chevronA, chevronB) * 0.38;
    float fill = 0.11 + 0.16 * (chevronA + chevronB);

    return vec4(primary, accent, highlight, fill);
}

void main() {
    vec2 uv = gl_FragCoord.xy / iResolution.xy;
    vec2 p = uv * 2.0 - 1.0;
    p.x *= iResolution.x / iResolution.y;

    float speed = max(iSpeed, 0.05);
    float t = iTime * (0.40 + speed * 1.35);

    float scaleMix = clamp(iScale / 100.0, 0.0, 1.0);
    float growth = clamp(iGrowth / 100.0, 0.0, 1.0);
    float glow = clamp(iEdgeGlow / 100.0, 0.0, 1.0);

    float density = mix(3.2, 14.0, scaleMix);
    float width = mix(0.090, 0.022, scaleMix);

    float theta = 0.18 * sin(t * 0.08);
    mat2 rot = mat2(cos(theta), -sin(theta), sin(theta), cos(theta));
    vec2 rp = rot * p;
    rp += vec2(sin(t * 0.17), cos(t * 0.13)) * 0.03;

    vec3 coords = latticeCoords(rp, density);
    vec3 axisDist = abs(fract(coords) - 0.5);
    vec2 local = fract(coords.xy) - 0.5;

    vec4 scene;
    if (iScene == 1) {
        scene = sceneShardfield(coords, axisDist, local, width, growth, t);
    } else if (iScene == 2) {
        scene = scenePrism(coords, axisDist, local, width, growth, t);
    } else if (iScene == 3) {
        scene = sceneSignal(coords, axisDist, local, width, growth, t);
    } else {
        scene = sceneLattice(coords, axisDist, local, width, growth, t);
    }

    float radial = length(p);
    float sweep = fract(t * (0.11 + growth * 0.12));
    float growthBand = 1.0 - smoothstep(0.02, 0.19, abs(radial * (1.2 + growth * 1.7) - sweep * 1.35));
    float center = 1.0 - smoothstep(0.12, 0.95, radial * (1.0 + growth * 0.8));
    float growthGain = (0.66 + growth * 0.45) + growthBand * (0.32 + growth * 0.34) + center * 0.12;
    scene.xyz *= growthGain;

    vec3 bgA = paletteColor(iPalette, 0);
    vec3 bgB = paletteColor(iPalette, 1);
    vec3 primaryColor = paletteColor(iPalette, 2);
    vec3 accentColor = paletteColor(iPalette, 3);
    vec3 highlightColor = paletteColor(iPalette, 4);

    float baseMix = clamp(scene.w * 0.72 + uv.y * 0.35, 0.0, 1.0);
    vec3 color = mix(bgA, bgB, baseMix);

    float primaryMask = clamp(scene.x, 0.0, 1.35);
    float accentMask = clamp(scene.y, 0.0, 1.35);
    float highlightMask = clamp(scene.z, 0.0, 1.55);

    float edgeGain = 0.42 + glow * 0.85;
    color += primaryColor * primaryMask * (0.46 + edgeGain * 0.42);
    color += accentColor * accentMask * (0.44 + edgeGain * 0.45);
    color += highlightColor * highlightMask * (0.40 + edgeGain * 0.36);

    float bloom = (
        primaryMask * primaryMask * 0.55 +
        accentMask * accentMask * 0.62 +
        highlightMask * highlightMask * 0.84
    ) * (0.16 + glow * 0.68);
    color += highlightColor * bloom * 0.34;

    float vignette = smoothstep(1.45, 0.18, radial);
    color *= 0.82 + vignette * 0.42;

    float scan = 0.95 + 0.05 * step(0.5, fract(gl_FragCoord.y * 0.5));
    color *= scan;

    color = color / (1.0 + color * 0.55);
    color = pow(clamp(color, 0.0, 1.0), vec3(1.02));
    fragColor = vec4(clamp(color, 0.0, 1.0), 1.0);
}
