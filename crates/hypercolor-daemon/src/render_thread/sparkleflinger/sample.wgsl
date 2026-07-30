struct Wide64 {
  lo: u32,
  hi: u32,
}

struct Wide128 {
  words: vec4<u32>,
}

struct WideRgb {
  r: Wide64,
  g: Wide64,
  b: Wide64,
}

struct WideRgb128 {
  r: Wide128,
  g: Wide128,
  b: Wide128,
}

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
@group(0) @binding(4) var<storage, read> summed_area: array<WideRgb>;

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

fn wide64_zero() -> Wide64 {
  return Wide64(0u, 0u);
}

fn wide64_sub(left: Wide64, right: Wide64) -> Wide64 {
  let borrow = select(0u, 1u, left.lo < right.lo);
  return Wide64(left.lo - right.lo, left.hi - right.hi - borrow);
}

fn wide128_zero() -> Wide128 {
  return Wide128(vec4<u32>(0u));
}

fn wide128_add(left: Wide128, right: Wide128) -> Wide128 {
  let word0 = left.words.x + right.words.x;
  let carry0 = select(0u, 1u, word0 < left.words.x);
  let partial1 = left.words.y + right.words.y;
  let carry1a = select(0u, 1u, partial1 < left.words.y);
  let word1 = partial1 + carry0;
  let carry1b = select(0u, 1u, word1 < partial1);
  let partial2 = left.words.z + right.words.z;
  let carry2a = select(0u, 1u, partial2 < left.words.z);
  let word2 = partial2 + carry1a + carry1b;
  let carry2b = select(0u, 1u, word2 < partial2);
  let word3 = left.words.w + right.words.w + carry2a + carry2b;
  return Wide128(vec4<u32>(word0, word1, word2, word3));
}

fn mul_u32(left: u32, right: u32) -> Wide64 {
  let left_lo = left & 0xffffu;
  let left_hi = left >> 16u;
  let right_lo = right & 0xffffu;
  let right_hi = right >> 16u;
  let product00 = left_lo * right_lo;
  let product01 = left_lo * right_hi;
  let product10 = left_hi * right_lo;
  let product11 = left_hi * right_hi;
  let digit0 = product00 & 0xffffu;
  let carry0 = product00 >> 16u;
  let digit1_sum = carry0 + (product01 & 0xffffu) + (product10 & 0xffffu);
  let digit1 = digit1_sum & 0xffffu;
  let digit2_sum = (digit1_sum >> 16u)
    + (product01 >> 16u)
    + (product10 >> 16u)
    + (product11 & 0xffffu);
  let digit2 = digit2_sum & 0xffffu;
  let digit3 = (digit2_sum >> 16u) + (product11 >> 16u);
  return Wide64(digit0 | (digit1 << 16u), digit2 | (digit3 << 16u));
}

fn wide64_mul(left: Wide64, right: Wide64) -> Wide128 {
  let product00 = mul_u32(left.lo, right.lo);
  let product01 = mul_u32(left.lo, right.hi);
  let product10 = mul_u32(left.hi, right.lo);
  let product11 = mul_u32(left.hi, right.hi);
  var result = Wide128(vec4<u32>(product00.lo, product00.hi, 0u, 0u));
  result = wide128_add(
    result,
    Wide128(vec4<u32>(0u, product01.lo, product01.hi, 0u)),
  );
  result = wide128_add(
    result,
    Wide128(vec4<u32>(0u, product10.lo, product10.hi, 0u)),
  );
  return wide128_add(
    result,
    Wide128(vec4<u32>(0u, 0u, product11.lo, product11.hi)),
  );
}

fn wide128_mul_u32(value: Wide128, multiplier: u32) -> Wide128 {
  let product0 = mul_u32(value.words.x, multiplier);
  let product1 = mul_u32(value.words.y, multiplier);
  let product2 = mul_u32(value.words.z, multiplier);
  let product3 = mul_u32(value.words.w, multiplier);
  var result = Wide128(vec4<u32>(product0.lo, product0.hi, 0u, 0u));
  result = wide128_add(
    result,
    Wide128(vec4<u32>(0u, product1.lo, product1.hi, 0u)),
  );
  result = wide128_add(
    result,
    Wide128(vec4<u32>(0u, 0u, product2.lo, product2.hi)),
  );
  return wide128_add(
    result,
    Wide128(vec4<u32>(0u, 0u, 0u, product3.lo)),
  );
}

fn wide128_less_equal(left: Wide128, right: Wide128) -> bool {
  if (left.words.w != right.words.w) {
    return left.words.w < right.words.w;
  }
  if (left.words.z != right.words.z) {
    return left.words.z < right.words.z;
  }
  if (left.words.y != right.words.y) {
    return left.words.y < right.words.y;
  }
  return left.words.x <= right.words.x;
}

fn divide_average(numerator: Wide128, denominator: Wide128) -> u32 {
  var quotient = 0u;
  var bit = 0x8000u;
  loop {
    if (bit == 0u) {
      break;
    }
    let candidate = quotient | bit;
    if (wide128_less_equal(wide128_mul_u32(denominator, candidate), numerator)) {
      quotient = candidate;
    }
    bit >>= 1u;
  }
  return quotient;
}

fn rgb64_zero() -> WideRgb {
  return WideRgb(wide64_zero(), wide64_zero(), wide64_zero());
}

fn rgb64_sub(left: WideRgb, right: WideRgb) -> WideRgb {
  return WideRgb(
    wide64_sub(left.r, right.r),
    wide64_sub(left.g, right.g),
    wide64_sub(left.b, right.b),
  );
}

fn rgb128_zero() -> WideRgb128 {
  return WideRgb128(wide128_zero(), wide128_zero(), wide128_zero());
}

fn rgb128_add_weighted(
  sum: WideRgb128,
  value: WideRgb,
  weight: Wide64,
) -> WideRgb128 {
  return WideRgb128(
    wide128_add(sum.r, wide64_mul(value.r, weight)),
    wide128_add(sum.g, wide64_mul(value.g, weight)),
    wide128_add(sum.b, wide64_mul(value.b, weight)),
  );
}

fn rectangle_sum(x0: u32, y0: u32, x1: u32, y1: u32) -> WideRgb {
  let bottom_right = summed_area[y1 * params.width + x1];
  var bottom_left = rgb64_zero();
  if (x0 > 0u) {
    bottom_left = summed_area[y1 * params.width + x0 - 1u];
  }
  var top_right = rgb64_zero();
  if (y0 > 0u) {
    top_right = summed_area[(y0 - 1u) * params.width + x1];
  }
  var top_left = rgb64_zero();
  if (x0 > 0u && y0 > 0u) {
    top_left = summed_area[(y0 - 1u) * params.width + x0 - 1u];
  }
  return rgb64_sub(
    rgb64_sub(bottom_right, bottom_left),
    rgb64_sub(top_right, top_left),
  );
}

fn radius_count(radius: u32) -> Wide64 {
  return Wide64((radius << 1u) | 1u, radius >> 31u);
}

fn sample_area_linear_u16(point: SamplePoint) -> vec3<u32> {
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
  let repeated_before_x = point.radius_x - before_x;
  let repeated_before_y = point.radius_y - before_y;
  let repeated_after_x = point.radius_x - after_x;
  let repeated_after_y = point.radius_y - after_y;

  var sum = rgb128_zero();
  sum = rgb128_add_weighted(
    sum,
    rectangle_sum(start_x, start_y, end_x, end_y),
    Wide64(1u, 0u),
  );
  sum = rgb128_add_weighted(
    sum,
    rectangle_sum(0u, start_y, 0u, end_y),
    Wide64(repeated_before_x, 0u),
  );
  sum = rgb128_add_weighted(
    sum,
    rectangle_sum(max_x, start_y, max_x, end_y),
    Wide64(repeated_after_x, 0u),
  );
  sum = rgb128_add_weighted(
    sum,
    rectangle_sum(start_x, 0u, end_x, 0u),
    Wide64(repeated_before_y, 0u),
  );
  sum = rgb128_add_weighted(
    sum,
    rectangle_sum(start_x, max_y, end_x, max_y),
    Wide64(repeated_after_y, 0u),
  );
  sum = rgb128_add_weighted(
    sum,
    rectangle_sum(0u, 0u, 0u, 0u),
    mul_u32(repeated_before_x, repeated_before_y),
  );
  sum = rgb128_add_weighted(
    sum,
    rectangle_sum(max_x, 0u, max_x, 0u),
    mul_u32(repeated_after_x, repeated_before_y),
  );
  sum = rgb128_add_weighted(
    sum,
    rectangle_sum(0u, max_y, 0u, max_y),
    mul_u32(repeated_before_x, repeated_after_y),
  );
  sum = rgb128_add_weighted(
    sum,
    rectangle_sum(max_x, max_y, max_x, max_y),
    mul_u32(repeated_after_x, repeated_after_y),
  );

  let count = wide64_mul(radius_count(point.radius_x), radius_count(point.radius_y));
  return vec3<u32>(
    divide_average(sum.r, count),
    divide_average(sum.g, count),
    divide_average(sum.b, count),
  );
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
      linear_u16 = (linear_u16 * point.attenuation + vec3<u32>(128u)) / 256u;
    }
    linear_rgb = vec3<f32>(linear_u16) / 65535.0;
  }
  let attenuation = point.attenuation;
  if (point.method != 2u && attenuation < 256u) {
    linear_rgb *= f32(attenuation) / 256.0;
  }
  output_rgb[index] = encode_linear_rgb(linear_rgb);
}
