struct SourceCopyParams {
    size_and_flags: vec4<u32>,
};

@group(0) @binding(0)
var source_texture: texture_2d<f32>;

@group(0) @binding(1)
var output_texture: texture_storage_2d<rgba8unorm, write>;

@group(0) @binding(2)
var<uniform> params: SourceCopyParams;

@compute @workgroup_size(8, 8, 1)
fn copy_source(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.size_and_flags.x || gid.y >= params.size_and_flags.y) {
        return;
    }

    let source_y = select(
        gid.y,
        params.size_and_flags.y - 1u - gid.y,
        params.size_and_flags.z != 0u,
    );
    let color = textureLoad(source_texture, vec2<i32>(i32(gid.x), i32(source_y)), 0);
    textureStore(output_texture, vec2<i32>(i32(gid.x), i32(gid.y)), color);
}
