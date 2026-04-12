struct PreviewScaleParams {
    size: vec4<u32>,
};

@group(0) @binding(0)
var source_texture: texture_2d<f32>;

@group(0) @binding(1)
var<storage, read_write> output_pixels: array<u32>;

@group(0) @binding(2)
var<uniform> params: PreviewScaleParams;

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

fn sample_source(x: u32, y: u32) -> vec4<f32> {
    return decode_srgb(textureLoad(source_texture, vec2<i32>(i32(x), i32(y)), 0));
}

fn pack_srgba8(color: vec4<f32>) -> u32 {
    let srgb = encode_srgb(color);
    let rgba8 = vec4<u32>(
        u32(clamp(srgb.r, 0.0, 1.0) * 255.0 + 0.5),
        u32(clamp(srgb.g, 0.0, 1.0) * 255.0 + 0.5),
        u32(clamp(srgb.b, 0.0, 1.0) * 255.0 + 0.5),
        u32(clamp(srgb.a, 0.0, 1.0) * 255.0 + 0.5),
    );
    return rgba8.r | (rgba8.g << 8u) | (rgba8.b << 16u) | (rgba8.a << 24u);
}

@compute @workgroup_size(8, 8, 1)
fn scale_preview(@builtin(global_invocation_id) gid: vec3<u32>) {
    let preview_width = params.size.z;
    let preview_height = params.size.w;
    if (gid.x >= preview_width || gid.y >= preview_height) {
        return;
    }

    let source_width = params.size.x;
    let source_height = params.size.y;

    let scale_x = f32(source_width) / f32(preview_width);
    let scale_y = f32(source_height) / f32(preview_height);
    let source_x = clamp((f32(gid.x) + 0.5) * scale_x - 0.5, 0.0, f32(source_width - 1u));
    let source_y = clamp((f32(gid.y) + 0.5) * scale_y - 0.5, 0.0, f32(source_height - 1u));

    let x0 = u32(floor(source_x));
    let y0 = u32(floor(source_y));
    let x1 = min(x0 + 1u, source_width - 1u);
    let y1 = min(y0 + 1u, source_height - 1u);
    let tx = source_x - f32(x0);
    let ty = source_y - f32(y0);

    let top = mix(sample_source(x0, y0), sample_source(x1, y0), tx);
    let bottom = mix(sample_source(x0, y1), sample_source(x1, y1), tx);
    let color = mix(top, bottom, ty);
    let pixel_index = gid.y * preview_width + gid.x;
    output_pixels[pixel_index] = pack_srgba8(color);
}
