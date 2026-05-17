struct ComposeParams {
    size_and_mode: vec4<u32>,
    source_and_flags: vec4<u32>,
    opacity: vec4<f32>,
    transform_a: vec4<f32>,
    adjust_a: vec4<f32>,
    adjust_b: vec4<f32>,
};

@group(0) @binding(0)
var destination_texture: texture_2d<f32>;

@group(0) @binding(1)
var source_texture: texture_2d<f32>;

@group(0) @binding(2)
var output_texture: texture_storage_2d<rgba8unorm, write>;

@group(0) @binding(3)
var<uniform> params: ComposeParams;

const MODE_REPLACE: u32 = 0u;
const MODE_ALPHA: u32 = 1u;
const MODE_ADD: u32 = 2u;
const MODE_SCREEN: u32 = 3u;
const MODE_MULTIPLY: u32 = 4u;
const MODE_OVERLAY: u32 = 5u;
const MODE_SOFT_LIGHT: u32 = 6u;
const MODE_COLOR_DODGE: u32 = 7u;
const MODE_DIFFERENCE: u32 = 8u;
const MODE_TINT: u32 = 9u;
const MODE_LUMA_REVEAL: u32 = 10u;

const FIT_CONTAIN: u32 = 0u;
const FIT_COVER: u32 = 1u;
const FIT_STRETCH: u32 = 2u;
const FIT_TILE: u32 = 3u;
const FIT_MIRROR: u32 = 4u;

fn srgb_to_linear(channel: f32) -> f32 {
    if (channel <= 0.04045) {
        return channel / 12.92;
    }
    return pow((channel + 0.055) / 1.055, 2.4);
}

fn linear_to_srgb(channel: f32) -> f32 {
    if (channel <= 0.0031308) {
        return channel * 12.92;
    }
    return 1.055 * pow(channel, 1.0 / 2.4) - 0.055;
}

fn decode_srgb(color: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(
        srgb_to_linear(color.r),
        srgb_to_linear(color.g),
        srgb_to_linear(color.b),
        color.a,
    );
}

fn encode_srgb(color: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(
        linear_to_srgb(clamp(color.r, 0.0, 1.0)),
        linear_to_srgb(clamp(color.g, 0.0, 1.0)),
        linear_to_srgb(clamp(color.b, 0.0, 1.0)),
        clamp(color.a, 0.0, 1.0),
    );
}

fn compose_alpha(destination: vec4<f32>, source: vec4<f32>, opacity: f32) -> vec4<f32> {
    let source_alpha = source.a * opacity;
    let inverse_alpha = 1.0 - source_alpha;
    let rgb = destination.rgb * inverse_alpha + source.rgb * source_alpha;
    let alpha = min(destination.a + source_alpha - destination.a * source_alpha, 1.0);
    return vec4<f32>(rgb, alpha);
}

fn compose_add(destination: vec4<f32>, source: vec4<f32>, opacity: f32) -> vec4<f32> {
    let source_alpha = source.a * opacity;
    let inverse_alpha = 1.0 - source_alpha;
    let rgb = destination.rgb * inverse_alpha + min(destination.rgb + source.rgb, vec3<f32>(1.0)) * source_alpha;
    let alpha = min(destination.a + source_alpha - destination.a * source_alpha, 1.0);
    return vec4<f32>(rgb, alpha);
}

fn compose_screen(destination: vec4<f32>, source: vec4<f32>, opacity: f32) -> vec4<f32> {
    let source_alpha = source.a * opacity;
    let inverse_alpha = 1.0 - source_alpha;
    let screen = vec3<f32>(1.0) - (vec3<f32>(1.0) - destination.rgb) * (vec3<f32>(1.0) - source.rgb);
    let rgb = destination.rgb * inverse_alpha + screen * source_alpha;
    let alpha = min(destination.a + source_alpha - destination.a * source_alpha, 1.0);
    return vec4<f32>(rgb, alpha);
}

fn compose_blend(destination: vec4<f32>, source: vec4<f32>, opacity: f32, mode: u32) -> vec4<f32> {
    let source_alpha = source.a * opacity;
    let inverse_alpha = 1.0 - source_alpha;
    var blended = source.rgb;
    if (mode == MODE_MULTIPLY) {
        blended = destination.rgb * source.rgb;
    } else if (mode == MODE_OVERLAY) {
        let low = 2.0 * destination.rgb * source.rgb;
        let high = vec3<f32>(1.0) - 2.0 * (vec3<f32>(1.0) - destination.rgb) * (vec3<f32>(1.0) - source.rgb);
        blended = select(high, low, destination.rgb < vec3<f32>(0.5));
    } else if (mode == MODE_SOFT_LIGHT) {
        let low = destination.rgb - (vec3<f32>(1.0) - 2.0 * source.rgb) * destination.rgb * (vec3<f32>(1.0) - destination.rgb);
        let high = destination.rgb + (2.0 * source.rgb - vec3<f32>(1.0)) * (sqrt(destination.rgb) - destination.rgb);
        blended = select(high, low, source.rgb < vec3<f32>(0.5));
    } else if (mode == MODE_COLOR_DODGE) {
        blended = min(destination.rgb / max(vec3<f32>(1.0) - source.rgb, vec3<f32>(0.0001)), vec3<f32>(1.0));
        blended = select(blended, vec3<f32>(1.0), source.rgb >= vec3<f32>(1.0));
    } else if (mode == MODE_DIFFERENCE) {
        blended = abs(destination.rgb - source.rgb);
    }
    let rgb = destination.rgb * inverse_alpha + blended * source_alpha;
    let alpha = min(destination.a + source_alpha - destination.a * source_alpha, 1.0);
    return vec4<f32>(rgb, alpha);
}

fn linear_rgb_luma(rgb: vec3<f32>) -> f32 {
    return clamp(rgb.r * 0.2126 + rgb.g * 0.7152 + rgb.b * 0.0722, 0.0, 1.0);
}

fn rgb_colorfulness(rgb: vec3<f32>) -> f32 {
    let min_channel = min(min(rgb.r, rgb.g), rgb.b);
    let max_channel = max(max(rgb.r, rgb.g), rgb.b);
    return clamp(max_channel - min_channel, 0.0, 1.0);
}

fn screen_blend_channel(base: f32, blend: f32) -> f32 {
    return 1.0 - (1.0 - base) * (1.0 - blend);
}

fn tint_channel(effect_channel: f32, face_channel: f32, neutral: f32, emission_strength: f32) -> f32 {
    let tint = neutral * (1.0 - 0.72) + max(face_channel, neutral * 0.75) * 0.72;
    return screen_blend_channel(effect_channel * tint, face_channel * emission_strength);
}

fn effect_tint_material(effect_rgb: vec3<f32>, face_rgb: vec3<f32>) -> vec3<f32> {
    let luma = linear_rgb_luma(face_rgb);
    let colorfulness = rgb_colorfulness(face_rgb);
    let neutral = clamp(0.18 * (1.0 - luma) + luma, 0.18, 1.0);
    let emission_strength = (1.0 - colorfulness) * luma * 0.12;
    return vec3<f32>(
        tint_channel(effect_rgb.r, face_rgb.r, neutral, emission_strength),
        tint_channel(effect_rgb.g, face_rgb.g, neutral, emission_strength),
        tint_channel(effect_rgb.b, face_rgb.b, neutral, emission_strength),
    );
}

fn compose_tint(destination: vec4<f32>, source: vec4<f32>, opacity: f32) -> vec4<f32> {
    let source_alpha = source.a * opacity;
    let material = effect_tint_material(destination.rgb, source.rgb);
    let rgb = destination.rgb * (1.0 - source_alpha) + material * source_alpha;
    return vec4<f32>(rgb, destination.a);
}

fn smoothstep01(edge0: f32, edge1: f32, x: f32) -> f32 {
    if (edge0 >= edge1) {
        return select(0.0, 1.0, x >= edge1);
    }
    let t = clamp((x - edge0) / (edge1 - edge0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn compose_luma_reveal(destination: vec4<f32>, source: vec4<f32>, opacity: f32) -> vec4<f32> {
    let source_alpha = source.a * opacity;
    let material = effect_tint_material(destination.rgb, source.rgb);
    let reveal = smoothstep01(0.18, 0.92, linear_rgb_luma(source.rgb));
    let inside = source.rgb * (1.0 - reveal) + material * reveal;
    let rgb = destination.rgb * (1.0 - source_alpha) + inside * source_alpha;
    return vec4<f32>(rgb, destination.a);
}

fn positive_mod(value: i32, modulus: i32) -> i32 {
    let remainder = value % modulus;
    if (remainder < 0) {
        return remainder + modulus;
    }
    return remainder;
}

fn repeated_axis(value: f32, extent: u32, mirror: bool) -> i32 {
    let index = i32(floor(value));
    let extent_i = i32(extent);
    if (!mirror || extent <= 1u) {
        return positive_mod(index, extent_i);
    }

    let period = extent_i * 2;
    let phase = positive_mod(index, period);
    if (phase < extent_i) {
        return phase;
    }
    return period - 1 - phase;
}

fn source_coord_for_destination(xy: vec2<u32>) -> vec3<f32> {
    let source_size = vec2<f32>(f32(params.source_and_flags.x), f32(params.source_and_flags.y));
    let target_size = vec2<f32>(f32(params.size_and_mode.x), f32(params.size_and_mode.y));
    let anchor = vec2<f32>(params.opacity.y, params.opacity.z) * target_size;
    let delta = vec2<f32>(f32(xy.x) + 0.5, f32(xy.y) + 0.5) - anchor;
    let scale = max(vec2<f32>(params.opacity.w, params.transform_a.x), vec2<f32>(0.01));
    let local = vec2<f32>(
        (params.transform_a.y * delta.x + params.transform_a.z * delta.y) / scale.x,
        (-params.transform_a.z * delta.x + params.transform_a.y * delta.y) / scale.y,
    );

    if (params.size_and_mode.w == FIT_TILE || params.size_and_mode.w == FIT_MIRROR) {
        let base = anchor + local;
        return vec3<f32>(
            f32(repeated_axis(base.x, params.source_and_flags.x, params.size_and_mode.w == FIT_MIRROR)),
            f32(repeated_axis(base.y, params.source_and_flags.y, params.size_and_mode.w == FIT_MIRROR)),
            1.0,
        );
    }

    var draw_size = target_size;
    var crop_origin = vec2<f32>(0.0);
    var crop_size = source_size;
    let source_aspect = source_size.x / source_size.y;
    let target_aspect = target_size.x / target_size.y;
    if (params.size_and_mode.w == FIT_CONTAIN) {
        if (target_aspect > source_aspect) {
            draw_size = vec2<f32>(target_size.y * source_aspect, target_size.y);
        } else {
            draw_size = vec2<f32>(target_size.x, target_size.x / source_aspect);
        }
    } else if (params.size_and_mode.w == FIT_COVER) {
        if (target_aspect > source_aspect) {
            crop_size.y = source_size.x / target_aspect;
            crop_origin.y = (source_size.y - crop_size.y) * 0.5;
        } else {
            crop_size.x = source_size.y * target_aspect;
            crop_origin.x = (source_size.x - crop_size.x) * 0.5;
        }
    }

    let uv = local / draw_size + vec2<f32>(0.5);
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    return vec3<f32>(crop_origin + uv * crop_size - vec2<f32>(0.5), 1.0);
}

fn load_source_rgba(xy: vec2<u32>) -> vec4<f32> {
    if (params.source_and_flags.z == 0u) {
        return textureLoad(source_texture, vec2<i32>(i32(xy.x), i32(xy.y)), 0);
    }

    let coord = source_coord_for_destination(xy);
    if (coord.z < 0.5) {
        return vec4<f32>(0.0);
    }
    let max_coord = vec2<f32>(f32(params.source_and_flags.x - 1u), f32(params.source_and_flags.y - 1u));
    let source_xy = vec2<i32>(round(clamp(coord.xy, vec2<f32>(0.0), max_coord)));
    return textureLoad(source_texture, source_xy, 0);
}

fn rgb_to_hsl(color: vec3<f32>) -> vec3<f32> {
    let max_channel = max(max(color.r, color.g), color.b);
    let min_channel = min(min(color.r, color.g), color.b);
    let lightness = (max_channel + min_channel) * 0.5;
    let delta = max_channel - min_channel;
    if (delta <= 0.000001) {
        return vec3<f32>(0.0, 0.0, lightness);
    }

    let saturation = select(
        delta / (max_channel + min_channel),
        delta / (2.0 - max_channel - min_channel),
        lightness > 0.5,
    );
    var hue = 0.0;
    if (abs(max_channel - color.r) <= 0.000001) {
        hue = (color.g - color.b) / delta;
    } else if (abs(max_channel - color.g) <= 0.000001) {
        hue = ((color.b - color.r) / delta) + 2.0;
    } else {
        hue = ((color.r - color.g) / delta) + 4.0;
    }
    return vec3<f32>(fract(hue / 6.0 + 1.0), saturation, lightness);
}

fn hue_to_rgb(p: f32, q: f32, hue_in: f32) -> f32 {
    let hue = fract(hue_in + 1.0);
    if (hue < 1.0 / 6.0) {
        return p + (q - p) * 6.0 * hue;
    }
    if (hue < 0.5) {
        return q;
    }
    if (hue < 2.0 / 3.0) {
        return p + (q - p) * (2.0 / 3.0 - hue) * 6.0;
    }
    return p;
}

fn hsl_to_rgb(hsl: vec3<f32>) -> vec3<f32> {
    if (hsl.y <= 0.000001) {
        return vec3<f32>(hsl.z);
    }
    let q = select(
        hsl.z + hsl.y - hsl.z * hsl.y,
        hsl.z * (1.0 + hsl.y),
        hsl.z < 0.5,
    );
    let p = 2.0 * hsl.z - q;
    return vec3<f32>(
        hue_to_rgb(p, q, hsl.x + 1.0 / 3.0),
        hue_to_rgb(p, q, hsl.x),
        hue_to_rgb(p, q, hsl.x - 1.0 / 3.0),
    );
}

fn apply_adjust(color: vec4<f32>) -> vec4<f32> {
    var rgb = color.rgb * params.adjust_a.x;
    if (abs(params.adjust_a.y - 1.0) > 0.000001 || abs(params.adjust_a.z) > 0.000001) {
        var hsl = rgb_to_hsl(rgb);
        hsl.x = fract(hsl.x + params.adjust_a.z / 6.28318530718 + 1.0);
        hsl.y = clamp(hsl.y * params.adjust_a.y, 0.0, 1.0);
        rgb = hsl_to_rgb(hsl);
    }
    if (abs(params.adjust_b.w) > 0.000001) {
        rgb = (rgb - vec3<f32>(0.5)) * (1.0 + params.adjust_b.w) + vec3<f32>(0.5);
    }
    let tint_strength = clamp(params.adjust_a.w, 0.0, 1.0);
    if (tint_strength > 0.0) {
        rgb = rgb * (1.0 - tint_strength) + clamp(params.adjust_b.rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * tint_strength;
    }
    return vec4<f32>(rgb, color.a);
}

@compute @workgroup_size(8, 8, 1)
fn compose(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.size_and_mode.x || gid.y >= params.size_and_mode.y) {
        return;
    }

    let xy = vec2<i32>(i32(gid.x), i32(gid.y));
    let destination = decode_srgb(textureLoad(destination_texture, xy, 0));
    let source = apply_adjust(decode_srgb(load_source_rgba(gid.xy)));

    var composed = source;
    if (params.size_and_mode.z == MODE_ALPHA) {
        composed = compose_alpha(destination, source, params.opacity.x);
    } else if (params.size_and_mode.z == MODE_ADD) {
        composed = compose_add(destination, source, params.opacity.x);
    } else if (params.size_and_mode.z == MODE_SCREEN) {
        composed = compose_screen(destination, source, params.opacity.x);
    } else if (params.size_and_mode.z == MODE_TINT) {
        composed = compose_tint(destination, source, params.opacity.x);
    } else if (params.size_and_mode.z == MODE_LUMA_REVEAL) {
        composed = compose_luma_reveal(destination, source, params.opacity.x);
    } else if (params.size_and_mode.z >= MODE_MULTIPLY) {
        composed = compose_blend(destination, source, params.opacity.x, params.size_and_mode.z);
    }

    textureStore(output_texture, xy, encode_srgb(composed));
}
