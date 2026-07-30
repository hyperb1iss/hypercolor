const WORKGROUP_SIZE: u32 = 256u;

struct Wide64 {
  lo: u32,
  hi: u32,
}

struct WideRgb {
  r: Wide64,
  g: Wide64,
  b: Wide64,
}

struct SatParams {
  width: u32,
  height: u32,
  horizontal_blocks: u32,
  vertical_blocks: u32,
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> values: array<WideRgb>;
@group(0) @binding(2) var<storage, read_write> horizontal_sums: array<WideRgb>;
@group(0) @binding(3) var<storage, read_write> vertical_sums: array<WideRgb>;
@group(0) @binding(4) var<uniform> params: SatParams;

var<workgroup> scan_values: array<WideRgb, 256>;

fn wide_zero() -> Wide64 {
  return Wide64(0u, 0u);
}

fn wide_add(left: Wide64, right: Wide64) -> Wide64 {
  let lo = left.lo + right.lo;
  let carry = select(0u, 1u, lo < left.lo);
  return Wide64(lo, left.hi + right.hi + carry);
}

fn rgb_zero() -> WideRgb {
  return WideRgb(wide_zero(), wide_zero(), wide_zero());
}

fn rgb_add(left: WideRgb, right: WideRgb) -> WideRgb {
  return WideRgb(
    wide_add(left.r, right.r),
    wide_add(left.g, right.g),
    wide_add(left.b, right.b),
  );
}

fn srgb_to_linear(channel: f32) -> f32 {
  if (channel <= 0.04045) {
    return channel / 12.92;
  }
  return pow((channel + 0.055) / 1.055, 2.4);
}

fn decode_source_pixel(x: u32, y: u32) -> WideRgb {
  let sample = textureLoad(source_tex, vec2<i32>(i32(x), i32(y)), 0).rgb;
  return WideRgb(
    Wide64(u32(round(srgb_to_linear(sample.r) * 65535.0)), 0u),
    Wide64(u32(round(srgb_to_linear(sample.g) * 65535.0)), 0u),
    Wide64(u32(round(srgb_to_linear(sample.b) * 65535.0)), 0u),
  );
}

fn scan_workgroup(lane: u32, value: WideRgb) -> WideRgb {
  scan_values[lane] = value;
  workgroupBarrier();
  var offset = 1u;
  loop {
    if (offset >= WORKGROUP_SIZE) {
      break;
    }
    var addend = rgb_zero();
    if (lane >= offset) {
      addend = scan_values[lane - offset];
    }
    workgroupBarrier();
    scan_values[lane] = rgb_add(scan_values[lane], addend);
    workgroupBarrier();
    offset *= 2u;
  }
  return scan_values[lane];
}

@compute @workgroup_size(256)
fn scan_horizontal_tiles(
  @builtin(workgroup_id) group: vec3<u32>,
  @builtin(local_invocation_id) local: vec3<u32>,
) {
  let x = group.x * WORKGROUP_SIZE + local.x;
  let y = group.y;
  var value = rgb_zero();
  if (x < params.width && y < params.height) {
    value = decode_source_pixel(x, y);
  }
  let prefix = scan_workgroup(local.x, value);
  if (x < params.width && y < params.height) {
    values[y * params.width + x] = prefix;
    if (local.x == WORKGROUP_SIZE - 1u || x == params.width - 1u) {
      horizontal_sums[y * params.horizontal_blocks + group.x] = prefix;
    }
  }
}

@compute @workgroup_size(256)
fn add_horizontal_blocks(
  @builtin(workgroup_id) group: vec3<u32>,
  @builtin(local_invocation_id) local: vec3<u32>,
) {
  let x = group.x * WORKGROUP_SIZE + local.x;
  let y = group.y;
  if (x >= params.width || y >= params.height || group.x == 0u) {
    return;
  }
  let index = y * params.width + x;
  values[index] = rgb_add(
    values[index],
    horizontal_sums[y * params.horizontal_blocks + group.x - 1u],
  );
}

@compute @workgroup_size(256)
fn scan_vertical_tiles(
  @builtin(workgroup_id) group: vec3<u32>,
  @builtin(local_invocation_id) local: vec3<u32>,
) {
  let x = group.x;
  let y = group.y * WORKGROUP_SIZE + local.x;
  var value = rgb_zero();
  if (x < params.width && y < params.height) {
    value = values[y * params.width + x];
  }
  let prefix = scan_workgroup(local.x, value);
  if (x < params.width && y < params.height) {
    values[y * params.width + x] = prefix;
    if (local.x == WORKGROUP_SIZE - 1u || y == params.height - 1u) {
      vertical_sums[x * params.vertical_blocks + group.y] = prefix;
    }
  }
}

@compute @workgroup_size(256)
fn add_vertical_blocks(
  @builtin(workgroup_id) group: vec3<u32>,
  @builtin(local_invocation_id) local: vec3<u32>,
) {
  let x = group.x;
  let y = group.y * WORKGROUP_SIZE + local.x;
  if (x >= params.width || y >= params.height || group.y == 0u) {
    return;
  }
  let index = y * params.width + x;
  values[index] = rgb_add(
    values[index],
    vertical_sums[x * params.vertical_blocks + group.y - 1u],
  );
}
