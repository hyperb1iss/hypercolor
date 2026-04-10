struct ComposeParams {
    size_and_mode: vec4<u32>,
    opacity: vec4<f32>,
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

@compute @workgroup_size(8, 8, 1)
fn compose(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.size_and_mode.x || gid.y >= params.size_and_mode.y) {
        return;
    }

    let xy = vec2<i32>(i32(gid.x), i32(gid.y));
    let destination = decode_srgb(textureLoad(destination_texture, xy, 0));
    let source = decode_srgb(textureLoad(source_texture, xy, 0));

    var composed = source;
    if (params.size_and_mode.z == MODE_ALPHA) {
        composed = compose_alpha(destination, source, params.opacity.x);
    }

    textureStore(output_texture, xy, encode_srgb(composed));
}
