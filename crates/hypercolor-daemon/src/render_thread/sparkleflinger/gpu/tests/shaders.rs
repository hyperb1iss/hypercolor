use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Wide64 {
    lo: u32,
    hi: u32,
}

impl Wide64 {
    fn from_u64(value: u64) -> Self {
        Self {
            lo: value as u32,
            hi: (value >> 32) as u32,
        }
    }

    fn as_u64(self) -> u64 {
        u64::from(self.lo) | (u64::from(self.hi) << 32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Wide128 {
    words: [u32; 4],
}

impl Wide128 {
    fn from_u128(value: u128) -> Self {
        Self {
            words: [
                value as u32,
                (value >> 32) as u32,
                (value >> 64) as u32,
                (value >> 96) as u32,
            ],
        }
    }

    const fn as_u128(self) -> u128 {
        self.words[0] as u128
            | ((self.words[1] as u128) << 32)
            | ((self.words[2] as u128) << 64)
            | ((self.words[3] as u128) << 96)
    }
}

fn shader_module(name: &str, source: &str) -> wgpu::naga::Module {
    let module = wgpu::naga::front::wgsl::parse_str(source).unwrap_or_else(|error| {
        panic!(
            "{name} failed WGSL parsing:\n{}",
            error.emit_to_string(source)
        )
    });
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("{name} failed Naga validation: {error:#?}"));
    module
}

fn assert_shader_surface(
    name: &str,
    source: &str,
    required_functions: &[&str],
    required_entry_points: &[&str],
) {
    let module = shader_module(name, source);
    let functions = module
        .functions
        .iter()
        .filter_map(|(_, function)| function.name.as_deref())
        .collect::<BTreeSet<_>>();
    let entry_points = module
        .entry_points
        .iter()
        .map(|entry| entry.name.as_str())
        .collect::<BTreeSet<_>>();

    for function in required_functions {
        assert!(
            functions.contains(function),
            "{name} is missing arithmetic function {function}"
        );
    }
    for entry_point in required_entry_points {
        assert!(
            entry_points.contains(entry_point),
            "{name} is missing compute entry point {entry_point}"
        );
    }
}

fn wide64_add(left: Wide64, right: Wide64) -> Wide64 {
    let lo = left.lo.wrapping_add(right.lo);
    let carry = u32::from(lo < left.lo);
    Wide64 {
        lo,
        hi: left.hi.wrapping_add(right.hi).wrapping_add(carry),
    }
}

fn wide64_sub(left: Wide64, right: Wide64) -> Wide64 {
    let borrow = u32::from(left.lo < right.lo);
    Wide64 {
        lo: left.lo.wrapping_sub(right.lo),
        hi: left.hi.wrapping_sub(right.hi).wrapping_sub(borrow),
    }
}

fn wide128_add(left: Wide128, right: Wide128) -> Wide128 {
    let word0 = left.words[0].wrapping_add(right.words[0]);
    let carry0 = u32::from(word0 < left.words[0]);
    let partial1 = left.words[1].wrapping_add(right.words[1]);
    let carry1a = u32::from(partial1 < left.words[1]);
    let word1 = partial1.wrapping_add(carry0);
    let carry1b = u32::from(word1 < partial1);
    let partial2 = left.words[2].wrapping_add(right.words[2]);
    let carry2a = u32::from(partial2 < left.words[2]);
    let word2 = partial2.wrapping_add(carry1a).wrapping_add(carry1b);
    let carry2b = u32::from(word2 < partial2);
    let word3 = left.words[3]
        .wrapping_add(right.words[3])
        .wrapping_add(carry2a)
        .wrapping_add(carry2b);
    Wide128 {
        words: [word0, word1, word2, word3],
    }
}

fn mul_u32(left: u32, right: u32) -> Wide64 {
    let left_lo = left & 0xffff;
    let left_hi = left >> 16;
    let right_lo = right & 0xffff;
    let right_hi = right >> 16;
    let product00 = left_lo * right_lo;
    let product01 = left_lo * right_hi;
    let product10 = left_hi * right_lo;
    let product11 = left_hi * right_hi;
    let digit0 = product00 & 0xffff;
    let carry0 = product00 >> 16;
    let digit1_sum = carry0 + (product01 & 0xffff) + (product10 & 0xffff);
    let digit1 = digit1_sum & 0xffff;
    let digit2_sum =
        (digit1_sum >> 16) + (product01 >> 16) + (product10 >> 16) + (product11 & 0xffff);
    let digit2 = digit2_sum & 0xffff;
    let digit3 = (digit2_sum >> 16) + (product11 >> 16);
    Wide64 {
        lo: digit0 | (digit1 << 16),
        hi: digit2 | (digit3 << 16),
    }
}

fn wide64_mul(left: Wide64, right: Wide64) -> Wide128 {
    let product00 = mul_u32(left.lo, right.lo);
    let product01 = mul_u32(left.lo, right.hi);
    let product10 = mul_u32(left.hi, right.lo);
    let product11 = mul_u32(left.hi, right.hi);
    let mut result = Wide128 {
        words: [product00.lo, product00.hi, 0, 0],
    };
    result = wide128_add(
        result,
        Wide128 {
            words: [0, product01.lo, product01.hi, 0],
        },
    );
    result = wide128_add(
        result,
        Wide128 {
            words: [0, product10.lo, product10.hi, 0],
        },
    );
    wide128_add(
        result,
        Wide128 {
            words: [0, 0, product11.lo, product11.hi],
        },
    )
}

fn wide128_mul_u32(value: Wide128, multiplier: u32) -> Wide128 {
    let product0 = mul_u32(value.words[0], multiplier);
    let product1 = mul_u32(value.words[1], multiplier);
    let product2 = mul_u32(value.words[2], multiplier);
    let product3 = mul_u32(value.words[3], multiplier);
    let mut result = Wide128 {
        words: [product0.lo, product0.hi, 0, 0],
    };
    result = wide128_add(
        result,
        Wide128 {
            words: [0, product1.lo, product1.hi, 0],
        },
    );
    result = wide128_add(
        result,
        Wide128 {
            words: [0, 0, product2.lo, product2.hi],
        },
    );
    wide128_add(
        result,
        Wide128 {
            words: [0, 0, 0, product3.lo],
        },
    )
}

fn wide128_less_equal(left: Wide128, right: Wide128) -> bool {
    if left.words[3] != right.words[3] {
        return left.words[3] < right.words[3];
    }
    if left.words[2] != right.words[2] {
        return left.words[2] < right.words[2];
    }
    if left.words[1] != right.words[1] {
        return left.words[1] < right.words[1];
    }
    left.words[0] <= right.words[0]
}

fn divide_average(numerator: Wide128, denominator: Wide128) -> u32 {
    let mut quotient = 0_u32;
    let mut bit = 0x8000_u32;
    while bit != 0 {
        let candidate = quotient | bit;
        if wide128_less_equal(wide128_mul_u32(denominator, candidate), numerator) {
            quotient = candidate;
        }
        bit >>= 1;
    }
    quotient
}

fn radius_count(radius: u32) -> Wide64 {
    Wide64 {
        lo: (radius << 1) | 1,
        hi: radius >> 31,
    }
}

#[test]
fn gpu_sampling_shaders_parse_validate_and_expose_exact_arithmetic() {
    assert_shader_surface(
        "area_sat.wgsl",
        include_str!("../../area_sat.wgsl"),
        &["wide_add", "scan_workgroup"],
        &[
            "scan_horizontal_tiles",
            "add_horizontal_blocks",
            "scan_vertical_tiles",
            "add_vertical_blocks",
        ],
    );
    assert_shader_surface(
        "area_hierarchy.wgsl",
        include_str!("../../area_hierarchy.wgsl"),
        &["wide_add", "scan_workgroup", "hierarchy_index"],
        &["scan_sum_tiles", "add_sum_offsets"],
    );
    assert_shader_surface(
        "sample.wgsl",
        include_str!("../../sample.wgsl"),
        &[
            "wide64_sub",
            "wide128_add",
            "mul_u32",
            "wide64_mul",
            "wide128_mul_u32",
            "wide128_less_equal",
            "divide_average",
            "radius_count",
            "rectangle_sum",
            "rgb128_add_weighted",
        ],
        &["sample_pixels"],
    );
}

#[test]
fn shader_limb_arithmetic_matches_native_integer_boundaries() {
    let u32_values = [0, 1, 0xffff, 0x1_0000, u32::MAX - 1, u32::MAX];
    for left in u32_values {
        for right in u32_values {
            assert_eq!(
                mul_u32(left, right).as_u64(),
                u64::from(left) * u64::from(right)
            );
        }
    }

    let u64_values = [
        0,
        1,
        u64::from(u32::MAX),
        u64::from(u32::MAX) + 1,
        u64::MAX - 1,
        u64::MAX,
    ];
    for left in u64_values {
        for right in u64_values {
            let left_wide = Wide64::from_u64(left);
            let right_wide = Wide64::from_u64(right);
            assert_eq!(
                wide64_add(left_wide, right_wide).as_u64(),
                left.wrapping_add(right)
            );
            assert_eq!(
                wide64_sub(left_wide, right_wide).as_u64(),
                left.wrapping_sub(right)
            );
            assert_eq!(
                wide64_mul(left_wide, right_wide).as_u128(),
                u128::from(left) * u128::from(right)
            );
        }
    }

    let u128_values = [
        0,
        1,
        u128::from(u64::MAX),
        u128::from(u64::MAX) + 1,
        u128::MAX - 1,
        u128::MAX,
    ];
    for left in u128_values {
        for right in u128_values {
            let left_wide = Wide128::from_u128(left);
            let right_wide = Wide128::from_u128(right);
            assert_eq!(
                wide128_add(left_wide, right_wide).as_u128(),
                left.wrapping_add(right)
            );
            assert_eq!(wide128_less_equal(left_wide, right_wide), left <= right);
        }
        for multiplier in u32_values {
            assert_eq!(
                wide128_mul_u32(Wide128::from_u128(left), multiplier).as_u128(),
                left.wrapping_mul(u128::from(multiplier))
            );
        }
    }
}

#[test]
fn shader_division_and_radius_counts_cover_full_u32_radii() {
    for radius in [0, 1, 65_535, 65_536, u32::MAX] {
        assert_eq!(radius_count(radius).as_u64(), u64::from(radius) * 2 + 1);
    }

    let max_radius_count = u128::from(u32::MAX) * 2 + 1;
    let denominators = [1_u128, 257, max_radius_count * max_radius_count];
    for denominator in denominators {
        for quotient in [0_u32, 1, 32_767, u16::MAX.into()] {
            for remainder in [0, denominator / 2, denominator - 1] {
                let numerator = denominator * u128::from(quotient) + remainder;
                assert_eq!(
                    divide_average(
                        Wide128::from_u128(numerator),
                        Wide128::from_u128(denominator)
                    ),
                    quotient
                );
            }
        }
    }
}
