struct SamplePoint {
  x: f32,
  y: f32,
  method: u32,
  attenuation: u32,
  center_x: u32,
  center_y: u32,
  radius_x: u32,
  radius_y: u32,
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
@group(0) @binding(4) var<storage, read> summed_area: array<vec4<f32>>;

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

fn summed_area_at(x: u32, y: u32) -> vec3<f32> {
  return summed_area[y * params.width + x].rgb;
}

fn rectangle_sum(x0: u32, y0: u32, x1: u32, y1: u32) -> vec3<f32> {
  var sum = summed_area_at(x1, y1);
  if (x0 > 0u) {
    sum -= summed_area_at(x0 - 1u, y1);
  }
  if (y0 > 0u) {
    sum -= summed_area_at(x1, y0 - 1u);
  }
  if (x0 > 0u && y0 > 0u) {
    sum += summed_area_at(x0 - 1u, y0 - 1u);
  }
  return sum;
}

fn sample_area_linear_u16(point: SamplePoint) -> vec3<f32> {
  let max_x = params.width - 1u;
  let max_y = params.height - 1u;
  let before_x = min(point.radius_x, point.center_x);
  let before_y = min(point.radius_y, point.center_y);
  let after_x = min(point.radius_x, max_x - point.center_x);
  let after_y = min(point.radius_y, max_y - point.center_y);
  let start_x = point.center_x - before_x;
  let start_y = point.center_y - before_y;
  let end_x = point.center_x + after_x;
  let end_y = point.center_y + after_y;
  let repeated_before_x = f32(point.radius_x - before_x);
  let repeated_before_y = f32(point.radius_y - before_y);
  let repeated_after_x = f32(point.radius_x - after_x);
  let repeated_after_y = f32(point.radius_y - after_y);

  var sum = rectangle_sum(start_x, start_y, end_x, end_y);
  sum += rectangle_sum(0u, start_y, 0u, end_y) * repeated_before_x;
  sum += rectangle_sum(max_x, start_y, max_x, end_y) * repeated_after_x;
  sum += rectangle_sum(start_x, 0u, end_x, 0u) * repeated_before_y;
  sum += rectangle_sum(start_x, max_y, end_x, max_y) * repeated_after_y;
  sum += rectangle_sum(0u, 0u, 0u, 0u) * repeated_before_x * repeated_before_y;
  sum += rectangle_sum(max_x, 0u, max_x, 0u) * repeated_after_x * repeated_before_y;
  sum += rectangle_sum(0u, max_y, 0u, max_y) * repeated_before_x * repeated_after_y;
  sum += rectangle_sum(max_x, max_y, max_x, max_y)
    * repeated_after_x
    * repeated_after_y;

  let count_x = f32(point.radius_x) * 2.0 + 1.0;
  let count_y = f32(point.radius_y) * 2.0 + 1.0;
  return floor(sum / (count_x * count_y));
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
  } else if (point.method == 1u) {
    linear_rgb = sample_bilinear_linear(position);
  } else {
    var linear_u16 = sample_area_linear_u16(point);
    if (point.attenuation < 256u) {
      linear_u16 = floor(
        (linear_u16 * f32(point.attenuation) + vec3<f32>(128.0)) / 256.0
      );
    }
    linear_rgb = linear_u16 / 65535.0;
  }
  let attenuation = point.attenuation;
  if (point.method != 2u && attenuation < 256u) {
    linear_rgb *= f32(attenuation) / 256.0;
  }
  output_rgb[index] = encode_linear_rgb(linear_rgb);
}
