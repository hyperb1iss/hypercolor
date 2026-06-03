use std::ops::Range;
use std::sync::OnceLock;

use hypercolor_types::canvas::{linear_to_output_u8, srgb_to_linear};

const LED_PERCEPTUAL_COMPENSATION_STRENGTH: f32 = 0.22;
const LED_NEUTRAL_COMPENSATION_WEIGHT: f32 = 0.25;
const LED_HEADROOM_WEIGHT_FLOOR: f32 = 0.1;

pub(super) fn prepare_output_for_led_ranges(
    colors: &mut [[u8; 3]],
    written_ranges: &[Range<usize>],
    brightness: f32,
) {
    let brightness = brightness.clamp(0.0, 1.0);
    let full_brightness = brightness >= 0.999;
    if brightness <= 0.0 {
        colors.fill([0, 0, 0]);
        return;
    }

    if written_ranges.is_empty() {
        return;
    }

    if written_ranges.len() == 1 {
        let range = &written_ranges[0];
        if range.start == 0 && range.end == colors.len() {
            prepare_output_for_leds(colors, brightness, full_brightness);
            return;
        }
    }

    for range in written_ranges {
        let start = range.start.min(colors.len());
        let end = range.end.min(colors.len());
        if start >= end {
            continue;
        }
        prepare_output_for_leds(&mut colors[start..end], brightness, full_brightness);
    }
}

fn prepare_output_for_leds(colors: &mut [[u8; 3]], brightness: f32, full_brightness: bool) {
    if full_brightness {
        prepare_output_for_leds_full_brightness(colors);
        return;
    }

    prepare_output_for_leds_scaled(colors, brightness);
}

fn prepare_output_for_leds_full_brightness(colors: &mut [[u8; 3]]) {
    for color in colors {
        let [red_u8, green_u8, blue_u8] = *color;
        if red_u8 == 0 && green_u8 == 0 && blue_u8 == 0 {
            continue;
        }

        let mut red = decode_srgb_channel(red_u8);
        let mut green = decode_srgb_channel(green_u8);
        let mut blue = decode_srgb_channel(blue_u8);
        apply_led_perceptual_compensation_channels(&mut red, &mut green, &mut blue);
        *color = [
            linear_to_output_u8(red),
            linear_to_output_u8(green),
            linear_to_output_u8(blue),
        ];
    }
}

/// Scale a zone's portion of the staging buffer by a per-zone brightness
/// multiplier. sRGB u8 space scaling: the device output brightness is applied
/// downstream in linear space by `prepare_output_for_leds_scaled`, so the final
/// output is `linear(sRGB * zone) * device`. Not strictly gamma-correct for the
/// zone factor, but the user tunes this by eye and the error is bounded at
/// moderate brightness values.
pub(super) fn apply_zone_brightness(colors: &mut [[u8; 3]], zone_brightness: f32) {
    if zone_brightness >= 0.999 {
        return;
    }
    if zone_brightness <= 0.0 {
        colors.fill([0, 0, 0]);
        return;
    }
    for color in colors {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            color[0] = (f32::from(color[0]) * zone_brightness) as u8;
            color[1] = (f32::from(color[1]) * zone_brightness) as u8;
            color[2] = (f32::from(color[2]) * zone_brightness) as u8;
        }
    }
}

fn prepare_output_for_leds_scaled(colors: &mut [[u8; 3]], brightness: f32) {
    for color in colors {
        let [red_u8, green_u8, blue_u8] = *color;
        if red_u8 == 0 && green_u8 == 0 && blue_u8 == 0 {
            continue;
        }

        let mut red = decode_srgb_channel(red_u8);
        let mut green = decode_srgb_channel(green_u8);
        let mut blue = decode_srgb_channel(blue_u8);
        apply_led_perceptual_compensation_channels(&mut red, &mut green, &mut blue);
        red *= brightness;
        green *= brightness;
        blue *= brightness;
        *color = [
            linear_to_output_u8(red),
            linear_to_output_u8(green),
            linear_to_output_u8(blue),
        ];
    }
}

fn apply_led_perceptual_compensation_channels(red: &mut f32, green: &mut f32, blue: &mut f32) {
    let max_channel = (*red).max(*green).max(*blue);
    if max_channel <= f32::EPSILON {
        return;
    }

    let min_channel = (*red).min(*green).min(*blue);
    let luma = red.mul_add(0.2126, green.mul_add(0.7152, *blue * 0.0722));
    let headroom = 1.0 - max_channel;
    if headroom <= f32::EPSILON {
        return;
    }

    // Point-light LEDs under-represent low-luma chromatic colors, especially
    // blue/cyan/magenta. Lift those gently while keeping neutrals closer to the
    // source and never exceeding the available channel headroom.
    let whiteness = min_channel / max_channel;
    let colorfulness = LED_NEUTRAL_COMPENSATION_WEIGHT
        + (1.0 - LED_NEUTRAL_COMPENSATION_WEIGHT) * (1.0 - whiteness);
    let shadow_bias = 1.0 - luma;
    let headroom_weight = LED_HEADROOM_WEIGHT_FLOOR + (1.0 - LED_HEADROOM_WEIGHT_FLOOR) * headroom;
    let gain = 1.0
        + LED_PERCEPTUAL_COMPENSATION_STRENGTH
            * shadow_bias
            * shadow_bias
            * headroom_weight
            * colorfulness;
    let gain = gain.min(1.0 / max_channel);

    if gain <= 1.0 {
        return;
    }

    *red = (*red * gain).min(1.0);
    *green = (*green * gain).min(1.0);
    *blue = (*blue * gain).min(1.0);
}

fn decode_srgb_channel(channel: u8) -> f32 {
    static SRGB_TO_LED_LINEAR_LUT: OnceLock<[f32; 256]> = OnceLock::new();

    SRGB_TO_LED_LINEAR_LUT.get_or_init(|| {
        std::array::from_fn(|index| {
            let srgb = f32::from(u8::try_from(index).expect("LUT index must fit in u8")) / 255.0;
            srgb_to_linear(srgb)
        })
    })[usize::from(channel)]
}
