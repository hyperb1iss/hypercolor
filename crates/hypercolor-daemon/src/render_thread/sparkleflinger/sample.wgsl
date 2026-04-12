struct SamplePoint {
  x: f32,
  y: f32,
  method: u32,
  attenuation: u32,
}

struct SampleParams {
  width: u32,
  height: u32,
  sample_count: u32,
  _pad: u32,
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var<storage, read> points: array<SamplePoint>;
@group(0) @binding(2) var<storage, read_write> output_rgb: array<u32>;
@group(0) @binding(3) var<uniform> params: SampleParams;

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

fn encode_srgb(rgb: vec3<f32>) -> u32 {
  let clamped = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
  let r = u32(round(clamped.x * 255.0));
  let g = u32(round(clamped.y * 255.0));
  let b = u32(round(clamped.z * 255.0));
  return r | (g << 8u) | (b << 16u) | (255u << 24u);
}

fn encode_linear_rgb(rgb: vec3<f32>) -> u32 {
  return encode_srgb(vec3<f32>(
    linear_to_srgb(rgb.x),
    linear_to_srgb(rgb.y),
    linear_to_srgb(rgb.z),
  ));
}

fn sample_nearest_linear(position: vec2<f32>) -> vec3<f32> {
  let max_x = max(params.width - 1u, 0u);
  let max_y = max(params.height - 1u, 0u);
  let fx = round(position.x * f32(max_x));
  let fy = round(position.y * f32(max_y));
  let sample = textureLoad(
    source_tex,
    vec2<i32>(
      i32(clamp(fx, 0.0, f32(max_x))),
      i32(clamp(fy, 0.0, f32(max_y))),
    ),
    0
  );
  return vec3<f32>(
    srgb_to_linear(sample.r),
    srgb_to_linear(sample.g),
    srgb_to_linear(sample.b),
  );
}

fn sample_bilinear_linear(position: vec2<f32>) -> vec3<f32> {
  let max_x = max(params.width - 1u, 0u);
  let max_y = max(params.height - 1u, 0u);
  let fx = position.x * f32(max_x);
  let fy = position.y * f32(max_y);
  let x0 = u32(floor(fx));
  let y0 = u32(floor(fy));
  let x1 = min(x0 + 1u, max_x);
  let y1 = min(y0 + 1u, max_y);
  let tx = fract(fx);
  let ty = fract(fy);

  let top_left = textureLoad(source_tex, vec2<i32>(i32(x0), i32(y0)), 0).rgb;
  let top_right = textureLoad(source_tex, vec2<i32>(i32(x1), i32(y0)), 0).rgb;
  let bottom_left = textureLoad(source_tex, vec2<i32>(i32(x0), i32(y1)), 0).rgb;
  let bottom_right = textureLoad(source_tex, vec2<i32>(i32(x1), i32(y1)), 0).rgb;

  let linear_top = mix(
    vec3<f32>(
      srgb_to_linear(top_left.x),
      srgb_to_linear(top_left.y),
      srgb_to_linear(top_left.z),
    ),
    vec3<f32>(
      srgb_to_linear(top_right.x),
      srgb_to_linear(top_right.y),
      srgb_to_linear(top_right.z),
    ),
    tx,
  );
  let linear_bottom = mix(
    vec3<f32>(
      srgb_to_linear(bottom_left.x),
      srgb_to_linear(bottom_left.y),
      srgb_to_linear(bottom_left.z),
    ),
    vec3<f32>(
      srgb_to_linear(bottom_right.x),
      srgb_to_linear(bottom_right.y),
      srgb_to_linear(bottom_right.z),
    ),
    tx,
  );
  return mix(linear_top, linear_bottom, ty);
}

@compute @workgroup_size(64)
fn sample_pixels(@builtin(global_invocation_id) gid: vec3<u32>) {
  let index = gid.x;
  if (index >= params.sample_count) {
    return;
  }

  let point = points[index];
  let position = vec2<f32>(
    clamp(point.x, 0.0, 1.0),
    clamp(point.y, 0.0, 1.0),
  );

  var linear_rgb: vec3<f32>;
  if (point.method == 0u) {
    linear_rgb = sample_nearest_linear(position);
  } else {
    linear_rgb = sample_bilinear_linear(position);
  }
  if (point.attenuation < 256u) {
    linear_rgb *= f32(point.attenuation) / 256.0;
  }
  output_rgb[index] = encode_linear_rgb(linear_rgb);
}
