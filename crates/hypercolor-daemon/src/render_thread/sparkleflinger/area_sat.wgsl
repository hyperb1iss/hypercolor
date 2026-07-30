const WORKGROUP_SIZE: u32 = 256u;

struct SatParams {
  width: u32,
  height: u32,
  horizontal_blocks: u32,
  vertical_blocks: u32,
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> values_a: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> values_b: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> horizontal_sums: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> vertical_sums: array<vec4<f32>>;
@group(0) @binding(5) var<uniform> params: SatParams;

var<workgroup> scan_values: array<vec4<f32>, 256>;

fn srgb_to_linear(channel: f32) -> f32 {
  if (channel <= 0.04045) {
    return channel / 12.92;
  }
  return pow((channel + 0.055) / 1.055, 2.4);
}

fn decode_source_pixel(x: u32, y: u32) -> vec4<f32> {
  let sample = textureLoad(source_tex, vec2<i32>(i32(x), i32(y)), 0).rgb;
  return vec4<f32>(
    round(srgb_to_linear(sample.r) * 65535.0),
    round(srgb_to_linear(sample.g) * 65535.0),
    round(srgb_to_linear(sample.b) * 65535.0),
    0.0,
  );
}

fn scan_workgroup(lane: u32, value: vec4<f32>) -> vec4<f32> {
  scan_values[lane] = value;
  workgroupBarrier();
  var offset = 1u;
  loop {
    if (offset >= WORKGROUP_SIZE) {
      break;
    }
    var addend = vec4<f32>(0.0);
    if (lane >= offset) {
      addend = scan_values[lane - offset];
    }
    workgroupBarrier();
    scan_values[lane] += addend;
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
  var value = vec4<f32>(0.0);
  if (x < params.width && y < params.height) {
    value = decode_source_pixel(x, y);
  }
  let prefix = scan_workgroup(local.x, value);
  if (x < params.width && y < params.height) {
    values_a[y * params.width + x] = prefix;
    if (local.x == WORKGROUP_SIZE - 1u || x == params.width - 1u) {
      horizontal_sums[y * params.horizontal_blocks + group.x] = prefix;
    }
  }
}

@compute @workgroup_size(256)
fn scan_horizontal_blocks(
  @builtin(workgroup_id) group: vec3<u32>,
  @builtin(local_invocation_id) local: vec3<u32>,
) {
  let y = group.y;
  var value = vec4<f32>(0.0);
  if (local.x < params.horizontal_blocks && y < params.height) {
    value = horizontal_sums[y * params.horizontal_blocks + local.x];
  }
  let prefix = scan_workgroup(local.x, value);
  if (local.x < params.horizontal_blocks && y < params.height) {
    horizontal_sums[y * params.horizontal_blocks + local.x] = prefix;
  }
}

@compute @workgroup_size(256)
fn add_horizontal_blocks(
  @builtin(workgroup_id) group: vec3<u32>,
  @builtin(local_invocation_id) local: vec3<u32>,
) {
  let x = group.x * WORKGROUP_SIZE + local.x;
  let y = group.y;
  if (x >= params.width || y >= params.height) {
    return;
  }
  var value = values_a[y * params.width + x];
  if (group.x > 0u) {
    value += horizontal_sums[y * params.horizontal_blocks + group.x - 1u];
  }
  values_b[y * params.width + x] = value;
}

@compute @workgroup_size(256)
fn scan_vertical_tiles(
  @builtin(workgroup_id) group: vec3<u32>,
  @builtin(local_invocation_id) local: vec3<u32>,
) {
  let x = group.x;
  let y = group.y * WORKGROUP_SIZE + local.x;
  var value = vec4<f32>(0.0);
  if (x < params.width && y < params.height) {
    value = values_b[y * params.width + x];
  }
  let prefix = scan_workgroup(local.x, value);
  if (x < params.width && y < params.height) {
    values_a[y * params.width + x] = prefix;
    if (local.x == WORKGROUP_SIZE - 1u || y == params.height - 1u) {
      vertical_sums[x * params.vertical_blocks + group.y] = prefix;
    }
  }
}

@compute @workgroup_size(256)
fn scan_vertical_blocks(
  @builtin(workgroup_id) group: vec3<u32>,
  @builtin(local_invocation_id) local: vec3<u32>,
) {
  let x = group.x;
  var value = vec4<f32>(0.0);
  if (local.x < params.vertical_blocks && x < params.width) {
    value = vertical_sums[x * params.vertical_blocks + local.x];
  }
  let prefix = scan_workgroup(local.x, value);
  if (local.x < params.vertical_blocks && x < params.width) {
    vertical_sums[x * params.vertical_blocks + local.x] = prefix;
  }
}

@compute @workgroup_size(256)
fn add_vertical_blocks(
  @builtin(workgroup_id) group: vec3<u32>,
  @builtin(local_invocation_id) local: vec3<u32>,
) {
  let x = group.x;
  let y = group.y * WORKGROUP_SIZE + local.x;
  if (x >= params.width || y >= params.height) {
    return;
  }
  var value = values_a[y * params.width + x];
  if (group.y > 0u) {
    value += vertical_sums[x * params.vertical_blocks + group.y - 1u];
  }
  values_b[y * params.width + x] = value;
}
