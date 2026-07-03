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

// Coefficients below are least-squares cosine fits to the Oklab-interpolated
// `stops` gradients in shared/palettes.json — keep them in sync when stops change.
vec3 iqPaletteSunset(float t) {
    return iqPalette(t, vec3(0.96, 0.61, 0.117), vec3(0.055, 0.5, 0.264), vec3(1.086, 0.47, 0.52), vec3(0.796, 0.298, 0.748));
}

vec3 iqPaletteCyberpunk(float t) {
    return iqPalette(t, vec3(0.736, 0.489, 0.797), vec3(0.269, 0.399, 0.276), vec3(1.586, 1.028, 1.068), vec3(0.976, 0.654, 0.8));
}

vec3 iqPaletteSilkCircuit(float t) {
    return iqPalette(t, vec3(0.787, 0.615, 0.611), vec3(0.322, 0.268, 0.335), vec3(1.3, 1.63, 0.614), vec3(0.19, 0.608, 0.964));
}
