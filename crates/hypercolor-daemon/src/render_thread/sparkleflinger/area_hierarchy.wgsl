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

struct HierarchyParams {
  level_offset: u32,
  next_offset: u32,
  level_length: u32,
  next_length: u32,
}

@group(0) @binding(0) var<storage, read_write> sums: array<WideRgb>;
@group(0) @binding(1) var<uniform> params: HierarchyParams;

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

fn hierarchy_index(offset: u32, segment: u32, length: u32, element: u32) -> u32 {
  return offset + segment * length + element;
}

@compute @workgroup_size(256)
fn scan_sum_tiles(
  @builtin(workgroup_id) group: vec3<u32>,
  @builtin(local_invocation_id) local: vec3<u32>,
) {
  let element = group.x * WORKGROUP_SIZE + local.x;
  let segment = group.y;
  var value = rgb_zero();
  if (element < params.level_length) {
    value = sums[hierarchy_index(
      params.level_offset,
      segment,
      params.level_length,
      element,
    )];
  }
  let prefix = scan_workgroup(local.x, value);
  if (element >= params.level_length) {
    return;
  }
  sums[hierarchy_index(
    params.level_offset,
    segment,
    params.level_length,
    element,
  )] = prefix;
  if (
    params.next_length > 0u
      && (local.x == WORKGROUP_SIZE - 1u || element == params.level_length - 1u)
  ) {
    sums[hierarchy_index(
      params.next_offset,
      segment,
      params.next_length,
      group.x,
    )] = prefix;
  }
}

@compute @workgroup_size(256)
fn add_sum_offsets(
  @builtin(workgroup_id) group: vec3<u32>,
  @builtin(local_invocation_id) local: vec3<u32>,
) {
  let element = group.x * WORKGROUP_SIZE + local.x;
  if (element >= params.level_length || group.x == 0u) {
    return;
  }
  let segment = group.y;
  let index = hierarchy_index(
    params.level_offset,
    segment,
    params.level_length,
    element,
  );
  let offset = hierarchy_index(
    params.next_offset,
    segment,
    params.next_length,
    group.x - 1u,
  );
  sums[index] = rgb_add(sums[index], sums[offset]);
}
