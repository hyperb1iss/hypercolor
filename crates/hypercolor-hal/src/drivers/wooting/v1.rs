//! v1 colour buffers: per-chip PWM layout behind the Atmel-VID firmware
//! (Spec 90 §4).

use std::sync::LazyLock;

use hypercolor_color::srgb_to_linear;

use super::types::{MATRIX_COLS, MATRIX_ROWS, V1_PART_PAYLOAD_LEN};

/// Marks a matrix cell with no LED behind it.
pub const NO_LED: u8 = u8::MAX;

/// LEDs one v1 colour report addresses.
pub const LEDS_PER_PART: usize = 24;

/// Most v1 colour reports a frame needs (the Two's numpad adds the fifth).
pub const MAX_PARTS: usize = 5;

/// LED index the ANSI Enter key drives.
pub const LED_ENTER_ANSI: u8 = 65;

/// LED index the ISO Enter key drives, mirrored from [`LED_ENTER_ANSI`].
pub const LED_ENTER_ISO: u8 = 62;

/// LED index the ANSI left Shift drives.
pub const LED_LEFT_SHIFT_ANSI: u8 = 9;

/// LED index the ISO left Shift drives, mirrored from [`LED_LEFT_SHIFT_ANSI`].
pub const LED_LEFT_SHIFT_ISO: u8 = 7;

/// Offset of the green PWM register from the red one.
pub const GREEN_OFFSET: usize = 0x10;

/// Offset of the blue PWM register from the red one.
pub const BLUE_OFFSET: usize = 0x20;

const N: u8 = NO_LED;

/// Driver-chip LED index behind each cell of the 6 x 21 key matrix.
pub const LED_INDEX: [[u8; MATRIX_COLS]; MATRIX_ROWS] = [
    [
        0, N, 11, 12, 23, 24, 36, 47, 85, 84, 49, 48, 59, 61, 73, 81, 80, 113, 114, 115, 116,
    ],
    [
        2, 1, 14, 13, 26, 25, 35, 38, 37, 87, 86, 95, 51, 63, 75, 72, 74, 96, 97, 98, 99,
    ],
    [
        3, 4, 15, 16, 27, 28, 39, 42, 40, 88, 89, 52, 53, 71, 76, 83, 77, 102, 103, 104, 100,
    ],
    [
        5, 6, 17, 18, 29, 30, 41, 46, 44, 90, 93, 54, 57, 65, N, N, N, 105, 106, 107, N,
    ],
    [
        9, 8, 19, 20, 31, 34, 32, 45, 43, 91, 92, 55, N, 66, N, 78, N, 108, 109, 110, 101,
    ],
    [
        10, 22, 21, N, N, N, 33, N, N, N, 94, 58, 67, 68, 70, 79, 82, N, 111, 112, N,
    ],
];

/// Red PWM register, inside a part's 96 bytes, for the LED at
/// `slot` (0..24) of that part.
///
/// A part holds two 48-byte banks of twelve LEDs. Inside a bank, LEDs
/// 0..6 sit at registers 0..6 and LEDs 6..12 at 8..14; green and blue
/// follow red at `+0x10` and `+0x20`.
#[must_use]
pub const fn red_register(slot: usize) -> usize {
    let bank = slot / 12;
    let within = slot % 12;
    bank * 0x30 + (within / 6) * 8 + within % 6
}

/// LED driver chip and starting register for a part.
#[must_use]
pub const fn part_address(part: usize) -> (u8, u8) {
    // Parts pair up per chip: the low half starts at register 0, the high
    // half at register 96.
    let chip = (part / 2) as u8;
    let register = if part.is_multiple_of(2) {
        0
    } else {
        V1_PART_PAYLOAD_LEN as u8
    };
    (chip, register)
}

static PWM_LEVELS: LazyLock<[u8; 256]> = LazyLock::new(|| {
    let mut table = [0_u8; 256];
    for (level, entry) in table.iter_mut().enumerate() {
        let linear = srgb_to_linear(level as f32 / 255.0);
        *entry = (linear * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    table
});

/// PWM duty for an sRGB channel byte.
///
/// v1 firmware writes PWM duty straight to the LED drivers, so the encoder
/// decodes the sRGB transfer curve first; v2 and newer firmware take
/// colour values and correct on the board.
#[must_use]
pub fn pwm_level(srgb: u8) -> u8 {
    PWM_LEVELS[usize::from(srgb)]
}

/// Build the PWM payloads for one frame.
///
/// `colors` is row-major `rows` by `cols`, landing on the key matrix from
/// row `first_row` down. LEDs above `led_index_max` are skipped, and the
/// ISO Enter and left Shift LEDs mirror their ANSI keys.
pub fn fill_parts(
    parts: &mut [[u8; V1_PART_PAYLOAD_LEN]; MAX_PARTS],
    colors: &[[u8; 3]],
    first_row: usize,
    rows: usize,
    cols: usize,
    led_index_max: u8,
) {
    for (row, indices) in LED_INDEX.iter().skip(first_row).take(rows).enumerate() {
        for (col, &led) in indices.iter().enumerate().take(cols) {
            if led == NO_LED || led > led_index_max {
                continue;
            }
            let Some(&color) = colors.get(row * cols + col) else {
                continue;
            };
            write_led(parts, led, color);
            match led {
                LED_ENTER_ANSI => write_led(parts, LED_ENTER_ISO, color),
                LED_LEFT_SHIFT_ANSI => write_led(parts, LED_LEFT_SHIFT_ISO, color),
                _ => {}
            }
        }
    }
}

fn write_led(parts: &mut [[u8; V1_PART_PAYLOAD_LEN]; MAX_PARTS], led: u8, [r, g, b]: [u8; 3]) {
    let led = usize::from(led);
    let Some(part) = parts.get_mut(led / LEDS_PER_PART) else {
        return;
    };
    let red = red_register(led % LEDS_PER_PART);
    part[red] = pwm_level(r);
    part[red + GREEN_OFFSET] = pwm_level(g);
    part[red + BLUE_OFFSET] = pwm_level(b);
}
