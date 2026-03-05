/**
 * Hypercolor GLSL Palette System
 * IQ cosine palette generator + named palette lookup.
 *
 * See: https://iquilezles.org/articles/palettes/
 */

// IQ procedural palette: offset + amplitude * cos(2pi * (frequency * t + phase))
vec3 iqPalette(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}

// Preset IQ palettes
vec3 iqPaletteRainbow(float t) {
    return iqPalette(t, vec3(0.5), vec3(0.5), vec3(1.0), vec3(0.0, 0.33, 0.67));
}

vec3 iqPaletteSunset(float t) {
    return iqPalette(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 1.0), vec3(0.0, 0.1, 0.2));
}

vec3 iqPaletteCyberpunk(float t) {
    return iqPalette(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 1.0, 0.5), vec3(0.8, 0.9, 0.3));
}

vec3 iqPaletteSilkCircuit(float t) {
    return iqPalette(t, vec3(0.5, 0.3, 0.5), vec3(0.5, 0.5, 0.5), vec3(1.0, 0.8, 0.6), vec3(0.85, 0.2, 0.5));
}
