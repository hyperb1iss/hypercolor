use super::average_channel;

#[test]
fn channel_average_handles_a_single_full_8k_reduction_box() {
    let samples = 7_680_u64 * 4_320;
    let sum = samples * u64::from(u8::MAX);

    assert!(sum > u64::from(u32::MAX));
    assert_eq!(average_channel(sum, samples), u8::MAX);
}

#[test]
fn channel_average_handles_the_maximum_d3d11_surface() {
    let samples = 16_384_u64 * 16_384;
    let sum = samples * u64::from(u8::MAX);

    assert_eq!(average_channel(sum, samples), u8::MAX);
}

#[test]
fn channel_average_defends_against_an_empty_box() {
    assert_eq!(average_channel(0, 0), 0);
}
