/**
 * Hypercolor GLSL Color Utilities
 * Color space conversions and manipulation functions.
 */

vec3 hsv2rgb(vec3 c) {
    vec4 K = vec4(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    vec3 p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
    return c.z * mix(K.xxx, clamp(p - K.xxx, 0.0, 1.0), c.y);
}

vec3 rgb2hsv(vec3 c) {
    vec4 K = vec4(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
    vec4 p = mix(vec4(c.bg, K.wz), vec4(c.gb, K.xy), step(c.b, c.g));
    vec4 q = mix(vec4(p.xyw, c.r), vec4(c.r, p.yzx), step(p.x, c.r));
    float d = q.x - min(q.w, q.y);
    float e = 1.0e-10;
    return vec3(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
}

vec3 softClip(vec3 c, float k) {
    float kk = max(0.0, k);
    return c / (1.0 + kk * c);
}

vec3 blendOverlay(vec3 base, vec3 blend) {
    return mix(
        2.0 * base * blend,
        1.0 - 2.0 * (1.0 - base) * (1.0 - blend),
        step(0.5, base)
    );
}

vec3 blendSoftLight(vec3 base, vec3 blend) {
    return mix(
        2.0 * base * blend + base * base * (1.0 - 2.0 * blend),
        sqrt(base) * (2.0 * blend - 1.0) + 2.0 * base * (1.0 - blend),
        step(0.5, blend)
    );
}

vec3 saturateColor(vec3 color, float factor) {
    vec3 hsv = rgb2hsv(color);
    hsv.y = clamp(hsv.y * factor, 0.0, 1.0);
    return hsv2rgb(hsv);
}

vec3 limitWhiteness(vec3 color, float threshold) {
    float maxChannel = max(max(color.r, color.g), color.b);
    float minChannel = min(min(color.r, color.g), color.b);
    float whiteness = minChannel / (maxChannel + 0.001);

    if (whiteness > threshold) {
        float scale = threshold / whiteness;
        vec3 colorPart = color - minChannel;
        float newMin = minChannel * scale;
        return colorPart + newMin;
    }
    return color;
}
