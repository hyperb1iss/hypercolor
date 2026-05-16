struct DisplayFinalizeParams {
    output_size_flags_mode: vec4<u32>,
    source_sizes: vec4<u32>,
    brightness_edge: vec4<u32>,
    viewport_position_size: vec4<f32>,
    viewport_rotation_scale: vec4<f32>,
    yuv_layout: vec4<u32>,
};

@group(0) @binding(0)
var scene_texture: texture_2d<f32>;

@group(0) @binding(1)
var face_texture: texture_2d<f32>;

@group(0) @binding(2)
var output_texture: texture_storage_2d<rgba8unorm, write>;

@group(0) @binding(3)
var<uniform> params: DisplayFinalizeParams;

@group(0) @binding(4)
var<storage, read_write> output_yuv: array<atomic<u32>>;

const MODE_REPLACE: u32 = 0u;
const MODE_ALPHA: u32 = 1u;
const MODE_TINT: u32 = 2u;
const MODE_LUMA_REVEAL: u32 = 3u;
const MODE_ADD: u32 = 4u;
const MODE_SCREEN: u32 = 5u;
const MODE_MULTIPLY: u32 = 6u;
const MODE_OVERLAY: u32 = 7u;
const MODE_SOFT_LIGHT: u32 = 8u;
const MODE_COLOR_DODGE: u32 = 9u;
const MODE_DIFFERENCE: u32 = 10u;

const EDGE_CLAMP: u32 = 0u;
const EDGE_WRAP: u32 = 1u;
const EDGE_MIRROR: u32 = 2u;
const EDGE_FADE_TO_BLACK: u32 = 3u;

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

fn encode_srgb(color: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        linear_to_srgb(clamp(color.r, 0.0, 1.0)),
        linear_to_srgb(clamp(color.g, 0.0, 1.0)),
        linear_to_srgb(clamp(color.b, 0.0, 1.0)),
    );
}

fn wrap_edge(value: f32) -> f32 {
    return value - floor(value);
}

fn mirror_edge(value: f32) -> f32 {
    let period = value - floor(value * 0.5) * 2.0;
    if (period >= 1.0) {
        return 2.0 - period;
    }
    return period;
}

fn apply_edge(value: f32, edge_behavior: u32) -> f32 {
    if (edge_behavior == EDGE_WRAP) {
        return wrap_edge(value);
    }
    if (edge_behavior == EDGE_MIRROR) {
        return mirror_edge(value);
    }
    return clamp(value, 0.0, 1.0);
}

fn fade_attenuation(position: vec2<f32>, edge_behavior: u32, falloff: f32) -> f32 {
    if (edge_behavior != EDGE_FADE_TO_BLACK) {
        return 1.0;
    }

    var dx = 0.0;
    if (position.x < 0.0) {
        dx = -position.x;
    } else if (position.x > 1.0) {
        dx = position.x - 1.0;
    }

    var dy = 0.0;
    if (position.y < 0.0) {
        dy = -position.y;
    } else if (position.y > 1.0) {
        dy = position.y - 1.0;
    }

    let distance = sqrt(dx * dx + dy * dy);
    if (distance <= 0.0) {
        return 1.0;
    }
    return clamp(exp(-distance * falloff), 0.0, 1.0);
}

fn sample_srgba(
    source: texture_2d<f32>,
    source_size: vec2<u32>,
    position: vec2<f32>,
    edge_behavior: u32,
    fade_falloff: f32,
) -> vec4<f32> {
    let sample_position = vec2<f32>(
        apply_edge(position.x, edge_behavior),
        apply_edge(position.y, edge_behavior),
    );
    let max_x = source_size.x - 1u;
    let max_y = source_size.y - 1u;
    let source_x = sample_position.x * f32(max_x);
    let source_y = sample_position.y * f32(max_y);
    let x0 = u32(floor(source_x));
    let y0 = u32(floor(source_y));
    let x1 = min(x0 + 1u, max_x);
    let y1 = min(y0 + 1u, max_y);
    let tx = source_x - f32(x0);
    let ty = source_y - f32(y0);

    let top_left = textureLoad(source, vec2<i32>(i32(x0), i32(y0)), 0);
    let top_right = textureLoad(source, vec2<i32>(i32(x1), i32(y0)), 0);
    let bottom_left = textureLoad(source, vec2<i32>(i32(x0), i32(y1)), 0);
    let bottom_right = textureLoad(source, vec2<i32>(i32(x1), i32(y1)), 0);
    let top = mix(top_left, top_right, tx);
    let bottom = mix(bottom_left, bottom_right, tx);
    var sampled = mix(top, bottom, ty);
    let attenuation = fade_attenuation(position, edge_behavior, fade_falloff);
    return vec4<f32>(sampled.rgb * attenuation, sampled.a);
}

fn screen_blend(destination: f32, source: f32) -> f32 {
    return 1.0 - (1.0 - destination) * (1.0 - source);
}

fn linear_rgb_luma(rgb: vec3<f32>) -> f32 {
    return clamp(rgb.r * 0.2126 + rgb.g * 0.7152 + rgb.b * 0.0722, 0.0, 1.0);
}

fn rgb_colorfulness(rgb: vec3<f32>) -> f32 {
    let min_channel = min(rgb.r, min(rgb.g, rgb.b));
    let max_channel = max(rgb.r, max(rgb.g, rgb.b));
    return clamp(max_channel - min_channel, 0.0, 1.0);
}

fn effect_tint_material(effect_rgb: vec3<f32>, face_rgb: vec3<f32>) -> vec3<f32> {
    let luma = linear_rgb_luma(face_rgb);
    let colorfulness = rgb_colorfulness(face_rgb);
    let neutral = clamp(0.18 * (1.0 - luma) + luma, 0.18, 1.0);
    let emission_strength = (1.0 - colorfulness) * luma * 0.12;
    let tint = neutral * (1.0 - 0.72) + max(face_rgb, vec3<f32>(neutral * 0.75)) * 0.72;
    let filtered = effect_rgb * tint;
    return vec3<f32>(
        screen_blend(filtered.r, face_rgb.r * emission_strength),
        screen_blend(filtered.g, face_rgb.g * emission_strength),
        screen_blend(filtered.b, face_rgb.b * emission_strength),
    );
}

fn smoothstep_unit(edge0: f32, edge1: f32, x: f32) -> f32 {
    if (edge0 >= edge1) {
        if (x >= edge1) {
            return 1.0;
        }
        return 0.0;
    }
    let t = clamp((x - edge0) / (edge1 - edge0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn blend_mode_rgb(destination: vec3<f32>, source: vec3<f32>, mode: u32) -> vec3<f32> {
    if (mode == MODE_ADD) {
        return min(destination + source, vec3<f32>(1.0));
    }
    if (mode == MODE_SCREEN) {
        return vec3<f32>(
            screen_blend(destination.r, source.r),
            screen_blend(destination.g, source.g),
            screen_blend(destination.b, source.b),
        );
    }
    if (mode == MODE_MULTIPLY) {
        return destination * source;
    }
    if (mode == MODE_OVERLAY) {
        let low = 2.0 * destination * source;
        let high = vec3<f32>(1.0) - 2.0 * (vec3<f32>(1.0) - destination) * (vec3<f32>(1.0) - source);
        return select(high, low, destination < vec3<f32>(0.5));
    }
    if (mode == MODE_SOFT_LIGHT) {
        let low = destination - (vec3<f32>(1.0) - 2.0 * source) * destination * (vec3<f32>(1.0) - destination);
        let high = destination + (2.0 * source - vec3<f32>(1.0)) * (sqrt(destination) - destination);
        return select(high, low, source < vec3<f32>(0.5));
    }
    if (mode == MODE_COLOR_DODGE) {
        let dodged = destination / max(vec3<f32>(1.0) - source, vec3<f32>(0.000001));
        return select(min(dodged, vec3<f32>(1.0)), vec3<f32>(1.0), source >= vec3<f32>(1.0));
    }
    if (mode == MODE_DIFFERENCE) {
        return abs(destination - source);
    }
    return source;
}

fn compose_display(destination: vec4<f32>, source: vec4<f32>, mode: u32, opacity: f32) -> vec3<f32> {
    let source_alpha = source.a * opacity;
    if (mode == MODE_REPLACE) {
        return source.rgb * source_alpha;
    }
    if (source_alpha <= 0.0) {
        return destination.rgb;
    }

    let inverse_alpha = 1.0 - source_alpha;
    if (mode == MODE_TINT) {
        let material = effect_tint_material(destination.rgb, source.rgb);
        return destination.rgb * inverse_alpha + material * source_alpha;
    }
    if (mode == MODE_LUMA_REVEAL) {
        let material = effect_tint_material(destination.rgb, source.rgb);
        let reveal = smoothstep_unit(0.18, 0.92, linear_rgb_luma(source.rgb));
        let inside = source.rgb * (1.0 - reveal) + material * reveal;
        return destination.rgb * inverse_alpha + inside * source_alpha;
    }

    let blended = blend_mode_rgb(destination.rgb, source.rgb, mode);
    return destination.rgb * inverse_alpha + blended * source_alpha;
}

fn apply_output_policy_srgb8(color: vec3<f32>) -> vec3<u32> {
    let brightness_factor = f32(params.brightness_edge.x);
    let srgb = encode_srgb(color);
    let srgb8 = floor(clamp(srgb, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0 + vec3<f32>(0.5));
    let scaled8 = floor(srgb8 * brightness_factor / 255.0);
    return vec3<u32>(u32(scaled8.r), u32(scaled8.g), u32(scaled8.b));
}

fn apply_output_policy(color: vec3<f32>) -> vec4<f32> {
    let scaled8 = apply_output_policy_srgb8(color);
    return vec4<f32>(
        f32(scaled8.r) / 255.0,
        f32(scaled8.g) / 255.0,
        f32(scaled8.b) / 255.0,
        1.0,
    );
}

fn clamp_to_u8(value: f32) -> u32 {
    return u32(clamp(floor(value + 0.5), 0.0, 255.0));
}

fn rgb8_to_yuv(rgb: vec3<f32>) -> vec3<u32> {
    let y = clamp_to_u8(0.299 * rgb.r + 0.587 * rgb.g + 0.114 * rgb.b);
    let u = clamp_to_u8(-0.168736 * rgb.r - 0.331264 * rgb.g + 0.5 * rgb.b + 128.0);
    let v = clamp_to_u8(0.5 * rgb.r - 0.418688 * rgb.g - 0.081312 * rgb.b + 128.0);
    return vec3<u32>(y, u, v);
}

fn write_yuv_byte(offset: u32, value: u32) {
    let word_index = offset / 4u;
    let shift = (offset & 3u) * 8u;
    atomicOr(&output_yuv[word_index], (value & 255u) << shift);
}

fn inside_circular_mask(x: u32, y: u32, width: u32, height: u32) -> bool {
    if (params.output_size_flags_mode.z == 0u) {
        return true;
    }
    let radius = i32(min(width, height));
    let dx = i32(x * 2u + 1u) - i32(width);
    let dy = i32(y * 2u + 1u) - i32(height);
    return dx * dx + dy * dy <= radius * radius;
}

fn display_srgba8_at(pixel: vec2<u32>, output_width: u32, output_height: u32) -> vec4<u32> {
    if (!inside_circular_mask(pixel.x, pixel.y, output_width, output_height)) {
        return vec4<u32>(0u);
    }

    let local = vec2<f32>(
        (f32(pixel.x) + 0.5) / f32(output_width),
        (f32(pixel.y) + 0.5) / f32(output_height),
    );
    let viewport_position = params.viewport_position_size.xy;
    let viewport_size = params.viewport_position_size.zw;
    let viewport_cos = params.viewport_rotation_scale.x;
    let viewport_sin = params.viewport_rotation_scale.y;
    let viewport_scale = params.viewport_rotation_scale.z;
    let opacity = params.viewport_rotation_scale.w;
    let sx = (local.x - 0.5) * viewport_size.x * viewport_scale;
    let sy = (local.y - 0.5) * viewport_size.y * viewport_scale;
    let scene_position = vec2<f32>(
        viewport_position.x + sx * viewport_cos - sy * viewport_sin,
        viewport_position.y + sx * viewport_sin + sy * viewport_cos,
    );
    let edge_behavior = params.brightness_edge.y;
    let fade_falloff = bitcast<f32>(params.brightness_edge.z);
    let scene_srgba = sample_srgba(
        scene_texture,
        params.source_sizes.xy,
        scene_position,
        edge_behavior,
        fade_falloff,
    );
    let face_srgba = sample_srgba(
        face_texture,
        params.source_sizes.zw,
        local,
        EDGE_CLAMP,
        0.0,
    );
    let scene_linear = decode_srgb(vec4<f32>(scene_srgba.rgb, 1.0));
    let face_linear = decode_srgb(face_srgba);
    let mode = params.output_size_flags_mode.w;
    let color = compose_display(scene_linear, face_linear, mode, opacity);
    let rgb = apply_output_policy_srgb8(color);
    return vec4<u32>(rgb, 255u);
}

@compute @workgroup_size(8, 8, 1)
fn finalize_display(@builtin(global_invocation_id) gid: vec3<u32>) {
    let output_width = params.output_size_flags_mode.x;
    let output_height = params.output_size_flags_mode.y;
    if (gid.x >= output_width || gid.y >= output_height) {
        return;
    }

    let srgba8 = display_srgba8_at(gid.xy, output_width, output_height);
    textureStore(
        output_texture,
        vec2<i32>(i32(gid.x), i32(gid.y)),
        vec4<f32>(
            f32(srgba8.r) / 255.0,
            f32(srgba8.g) / 255.0,
            f32(srgba8.b) / 255.0,
            f32(srgba8.a) / 255.0,
        ),
    );
}

@compute @workgroup_size(8, 8, 1)
fn finalize_display_yuv420(@builtin(global_invocation_id) gid: vec3<u32>) {
    let output_width = params.output_size_flags_mode.x;
    let output_height = params.output_size_flags_mode.y;
    if (gid.x >= output_width || gid.y >= output_height) {
        return;
    }

    let srgba8 = display_srgba8_at(gid.xy, output_width, output_height);
    let rgb = vec3<f32>(f32(srgba8.r), f32(srgba8.g), f32(srgba8.b));
    let yuv = rgb8_to_yuv(rgb);
    let y_stride = params.yuv_layout.x;
    let uv_stride = params.yuv_layout.y;
    let y_plane_len = params.yuv_layout.z;
    let u_plane_len = params.yuv_layout.w;

    write_yuv_byte(gid.y * y_stride + gid.x, yuv.x);

    if ((gid.x & 1u) != 0u || (gid.y & 1u) != 0u) {
        return;
    }

    var accum = vec3<f32>(0.0);
    var count = 0.0;
    for (var dy = 0u; dy < 2u; dy = dy + 1u) {
        for (var dx = 0u; dx < 2u; dx = dx + 1u) {
            let sample = gid.xy + vec2<u32>(dx, dy);
            if (sample.x < output_width && sample.y < output_height) {
                let sample_srgba8 = display_srgba8_at(sample, output_width, output_height);
                accum = accum + vec3<f32>(
                    f32(sample_srgba8.r),
                    f32(sample_srgba8.g),
                    f32(sample_srgba8.b),
                );
                count = count + 1.0;
            }
        }
    }

    let chroma = rgb8_to_yuv(accum / max(count, 1.0));
    let uv_offset = (gid.y / 2u) * uv_stride + (gid.x / 2u);
    write_yuv_byte(y_plane_len + uv_offset, chroma.y);
    write_yuv_byte(y_plane_len + u_plane_len + uv_offset, chroma.z);
}
