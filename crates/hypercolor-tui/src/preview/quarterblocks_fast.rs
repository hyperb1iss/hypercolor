use image::Rgb;
use image::RgbImage;
use image::flat::{FlatSamples, SampleLayout};
use image::imageops::{FilterType, resize};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::state::CanvasFrame;

const SPACE: char = ' ';
const FULL_BLOCK: char = '█';
const UPPER_HALF: char = '▀';
const LOWER_HALF: char = '▄';
const LEFT_HALF: char = '▌';
const RIGHT_HALF: char = '▐';
const UPPER_LEFT: char = '▘';
const UPPER_RIGHT: char = '▝';
const LOWER_LEFT: char = '▖';
const LOWER_RIGHT: char = '▗';
const DIAGONAL_FALLING: char = '▚';
const DIAGONAL_RISING: char = '▞';
const THREE_UPPER_LEFT: char = '▛';
const THREE_UPPER_RIGHT: char = '▜';
const THREE_LOWER_LEFT: char = '▙';
const THREE_LOWER_RIGHT: char = '▟';

const TOP_LEFT: u8 = 0b0001;
const TOP_RIGHT: u8 = 0b0010;
const BOTTOM_LEFT: u8 = 0b0100;
const BOTTOM_RIGHT: u8 = 0b1000;
const FULL_MASK: u8 = TOP_LEFT | TOP_RIGHT | BOTTOM_LEFT | BOTTOM_RIGHT;

const QUARTERBLOCK_MASK_ORDER: [u8; 8] = [
    0,
    FULL_MASK,
    TOP_LEFT | BOTTOM_LEFT,
    TOP_RIGHT | BOTTOM_RIGHT,
    TOP_RIGHT | BOTTOM_LEFT,
    TOP_LEFT | BOTTOM_RIGHT,
    TOP_LEFT | TOP_RIGHT,
    BOTTOM_LEFT | BOTTOM_RIGHT,
];
const CHANNEL_WEIGHTS: [u32; 3] = [30, 59, 11];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QuarterblockCell {
    fg: [u8; 3],
    bg: [u8; 3],
    char: char,
}

impl QuarterblockCell {
    fn new(quadrants: [[u8; 3]; 4]) -> Self {
        let average = average_color(&quadrants, FULL_MASK);
        let mut best = Self {
            fg: average,
            bg: average,
            char: SPACE,
        };
        let mut best_score = candidate_score(&quadrants, 0, average, average);

        for &mask in &QUARTERBLOCK_MASK_ORDER[1..] {
            let (fg, bg) = palette_for_mask(&quadrants, mask);
            let score = candidate_score(&quadrants, mask, fg, bg);
            if score < best_score {
                best = Self {
                    fg,
                    bg,
                    char: mask_to_char(mask),
                };
                best_score = score;
            }
        }

        best
    }
}

#[derive(Debug)]
pub(crate) struct QuarterblocksFrame {
    rect: Rect,
    cells: Vec<QuarterblockCell>,
}

impl QuarterblocksFrame {
    pub(crate) fn new(frame: &CanvasFrame, area: Rect) -> Result<Self, String> {
        if area.width == 0 || area.height == 0 {
            return Ok(Self {
                rect: area,
                cells: Vec::new(),
            });
        }

        let sampled = sample_quarterblock_grid(frame, area)?;
        let target_width = usize::from(area.width);
        let target_height = usize::from(area.height);
        let capacity = target_width
            .checked_mul(target_height)
            .ok_or_else(|| "preview area too large".to_string())?;
        let mut cells = Vec::with_capacity(capacity);

        for y in 0..target_height {
            for x in 0..target_width {
                cells.push(QuarterblockCell::new(sampled_quadrants(&sampled, x, y)));
            }
        }

        Ok(Self { rect: area, cells })
    }

    pub(crate) fn area(&self) -> Rect {
        self.rect
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
        let width = area.width.min(self.rect.width);
        let height = area.height.min(self.rect.height);
        let row_width = usize::from(self.rect.width);

        for y in 0..height {
            let row_offset = usize::from(y) * row_width;
            for x in 0..width {
                let cell = self.cells[row_offset + usize::from(x)];
                if let Some(buf_cell) = buf.cell_mut((area.left() + x, area.top() + y)) {
                    buf_cell
                        .set_char(cell.char)
                        .set_fg(Color::Rgb(cell.fg[0], cell.fg[1], cell.fg[2]))
                        .set_bg(Color::Rgb(cell.bg[0], cell.bg[1], cell.bg[2]));
                }
            }
        }
    }
}

fn sample_quarterblock_grid(frame: &CanvasFrame, area: Rect) -> Result<RgbImage, String> {
    let frame_width = usize::from(frame.width);
    let frame_height = usize::from(frame.height);
    let expected_len = frame_width
        .checked_mul(frame_height)
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or_else(|| "preview dimensions overflow".to_string())?;

    if frame.pixels.len() != expected_len {
        return Err("invalid preview frame length".to_string());
    }

    let target_width = u32::from(area.width) * 2;
    let target_height = u32::from(area.height) * 2;
    let source = FlatSamples {
        samples: frame.pixels.as_ref(),
        layout: SampleLayout::row_major_packed(3, u32::from(frame.width), u32::from(frame.height)),
        color_hint: None,
    };
    let view = source
        .as_view::<Rgb<u8>>()
        .map_err(|error| format!("invalid preview frame layout: {error}"))?;

    Ok(resize(
        &view,
        target_width,
        target_height,
        FilterType::Triangle,
    ))
}

fn sampled_quadrants(sampled: &RgbImage, x: usize, y: usize) -> [[u8; 3]; 4] {
    let sampled_x = u32::try_from(x * 2).expect("x fits in u32");
    let sampled_y = u32::try_from(y * 2).expect("y fits in u32");

    [
        sampled.get_pixel(sampled_x, sampled_y).0,
        sampled.get_pixel(sampled_x + 1, sampled_y).0,
        sampled.get_pixel(sampled_x, sampled_y + 1).0,
        sampled.get_pixel(sampled_x + 1, sampled_y + 1).0,
    ]
}

fn palette_for_mask(quadrants: &[[u8; 3]; 4], mask: u8) -> ([u8; 3], [u8; 3]) {
    if mask == 0 {
        let average = average_color(quadrants, FULL_MASK);
        return (average, average);
    }

    if mask == FULL_MASK {
        let average = average_color(quadrants, FULL_MASK);
        return (average, average);
    }

    (
        average_color(quadrants, mask),
        average_color(quadrants, FULL_MASK ^ mask),
    )
}

fn average_color(quadrants: &[[u8; 3]; 4], mask: u8) -> [u8; 3] {
    let mut sums = [0u32; 3];
    let mut count = 0u32;

    for (index, quadrant) in quadrants.iter().enumerate() {
        if mask & (1u8 << index) == 0 {
            continue;
        }

        sums[0] += u32::from(quadrant[0]);
        sums[1] += u32::from(quadrant[1]);
        sums[2] += u32::from(quadrant[2]);
        count += 1;
    }

    if count == 0 {
        return [0, 0, 0];
    }

    [
        ((sums[0] + count / 2) / count) as u8,
        ((sums[1] + count / 2) / count) as u8,
        ((sums[2] + count / 2) / count) as u8,
    ]
}

fn approximation_error(quadrants: &[[u8; 3]; 4], mask: u8, fg: [u8; 3], bg: [u8; 3]) -> u32 {
    quadrants
        .iter()
        .enumerate()
        .map(|(index, quadrant)| {
            let palette = if mask & (1u8 << index) == 0 { bg } else { fg };
            weighted_squared_difference(*quadrant, palette)
        })
        .sum()
}

fn candidate_score(quadrants: &[[u8; 3]; 4], mask: u8, fg: [u8; 3], bg: [u8; 3]) -> u32 {
    approximation_error(quadrants, mask, fg, bg)
        .saturating_add(orientation_penalty(quadrants, mask))
}

fn orientation_penalty(quadrants: &[[u8; 3]; 4], mask: u8) -> u32 {
    if !matches!(mask, 3 | 12) {
        return 0;
    }

    let top = average_color(quadrants, TOP_LEFT | TOP_RIGHT);
    let bottom = average_color(quadrants, BOTTOM_LEFT | BOTTOM_RIGHT);
    let left = average_color(quadrants, TOP_LEFT | BOTTOM_LEFT);
    let right = average_color(quadrants, TOP_RIGHT | BOTTOM_RIGHT);
    let vertical_contrast = weighted_squared_difference(top, bottom);
    let horizontal_contrast = weighted_squared_difference(left, right);

    let contrast_penalty = if vertical_contrast < 24_000 {
        24_000 - vertical_contrast
    } else {
        0
    };
    let orientation_penalty = horizontal_contrast
        .saturating_mul(2)
        .saturating_sub(vertical_contrast);

    contrast_penalty.saturating_add(orientation_penalty)
}

fn weighted_squared_difference(actual: [u8; 3], expected: [u8; 3]) -> u32 {
    CHANNEL_WEIGHTS
        .iter()
        .zip(actual.into_iter().zip(expected))
        .map(|(weight, (actual, expected))| {
            let delta = i32::from(actual) - i32::from(expected);
            weight * u32::try_from(delta * delta).expect("squared difference is non-negative")
        })
        .sum()
}

fn mask_to_char(mask: u8) -> char {
    match mask {
        0 => SPACE,
        1 => UPPER_LEFT,
        2 => UPPER_RIGHT,
        3 => UPPER_HALF,
        4 => LOWER_LEFT,
        5 => LEFT_HALF,
        6 => DIAGONAL_RISING,
        7 => THREE_UPPER_LEFT,
        8 => LOWER_RIGHT,
        9 => DIAGONAL_FALLING,
        10 => RIGHT_HALF,
        11 => THREE_UPPER_RIGHT,
        12 => LOWER_HALF,
        13 => THREE_LOWER_LEFT,
        14 => THREE_LOWER_RIGHT,
        15 => FULL_BLOCK,
        _ => SPACE,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BOTTOM_LEFT, BOTTOM_RIGHT, FULL_MASK, QuarterblockCell, QuarterblocksFrame, TOP_LEFT,
        TOP_RIGHT, approximation_error, mask_to_char, orientation_penalty,
        sample_quarterblock_grid, sampled_quadrants,
    };
    use crate::state::CanvasFrame;
    use ratatui::layout::Rect;

    const HALF_BLOCK_MASKS: [u8; 4] = [
        0,
        TOP_LEFT | TOP_RIGHT,
        BOTTOM_LEFT | BOTTOM_RIGHT,
        FULL_MASK,
    ];

    fn exact_mask_quadrants(mask: u8, fg: [u8; 3], bg: [u8; 3]) -> [[u8; 3]; 4] {
        [TOP_LEFT, TOP_RIGHT, BOTTOM_LEFT, BOTTOM_RIGHT]
            .map(|quadrant| if mask & quadrant == 0 { bg } else { fg })
    }

    fn used_masks(frame: &CanvasFrame, area: Rect) -> Vec<u8> {
        let sampled =
            sample_quarterblock_grid(frame, area).expect("quarterblock sample should build");
        let mut masks = Vec::with_capacity(usize::from(area.width) * usize::from(area.height));

        for y in 0..usize::from(area.height) {
            for x in 0..usize::from(area.width) {
                let quadrants = sampled_quadrants(&sampled, x, y);
                masks.push(char_to_mask(QuarterblockCell::new(quadrants).char));
            }
        }

        masks
    }

    fn soft_circle_frame(width: u16, height: u16) -> CanvasFrame {
        let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * 3);
        let center_x = (f32::from(width) - 1.0) * 0.5;
        let center_y = (f32::from(height) - 1.0) * 0.5;
        let radius = f32::from(width.min(height)) * 0.32;

        for y in 0..height {
            for x in 0..width {
                let dx = f32::from(x) - center_x;
                let dy = f32::from(y) - center_y;
                let distance = (dx * dx + dy * dy).sqrt();
                let edge = ((radius + 1.5 - distance) / 1.5).clamp(0.0, 1.0);
                let bg = [18.0, 8.0, 28.0];
                let fg = [96.0, 210.0, 235.0];
                let color = [
                    (bg[0] + (fg[0] - bg[0]) * edge).round() as u8,
                    (bg[1] + (fg[1] - bg[1]) * edge).round() as u8,
                    (bg[2] + (fg[2] - bg[2]) * edge).round() as u8,
                ];
                pixels.extend_from_slice(&color);
            }
        }

        CanvasFrame {
            frame_number: 1,
            timestamp_ms: 0,
            width,
            height,
            pixels: bytes::Bytes::from(pixels),
        }
    }

    fn horizontal_gradient_frame(width: u16, height: u16) -> CanvasFrame {
        let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * 3);

        for _y in 0..height {
            for x in 0..width {
                let t = if width <= 1 {
                    0.0
                } else {
                    f32::from(x) / f32::from(width - 1)
                };
                let left = [20.0, 110.0, 255.0];
                let right = [255.0, 45.0, 190.0];
                let color = [
                    (left[0] + (right[0] - left[0]) * t).round() as u8,
                    (left[1] + (right[1] - left[1]) * t).round() as u8,
                    (left[2] + (right[2] - left[2]) * t).round() as u8,
                ];
                pixels.extend_from_slice(&color);
            }
        }

        CanvasFrame {
            frame_number: 1,
            timestamp_ms: 0,
            width,
            height,
            pixels: bytes::Bytes::from(pixels),
        }
    }

    fn total_quarterblock_error(frame: &CanvasFrame, area: Rect) -> u32 {
        let sampled =
            sample_quarterblock_grid(frame, area).expect("quarterblock sample should build");
        let mut total_error = 0;

        for y in 0..usize::from(area.height) {
            for x in 0..usize::from(area.width) {
                let quadrants = sampled_quadrants(&sampled, x, y);
                let cell = QuarterblockCell::new(quadrants);
                let mask = char_to_mask(cell.char);
                total_error += approximation_error(&quadrants, mask, cell.fg, cell.bg);
            }
        }

        total_error
    }

    fn total_halfblock_error(frame: &CanvasFrame, area: Rect) -> u32 {
        let sampled =
            sample_quarterblock_grid(frame, area).expect("quarterblock sample should build");
        let mut total_error = 0;

        for y in 0..usize::from(area.height) {
            for x in 0..usize::from(area.width) {
                let quadrants = sampled_quadrants(&sampled, x, y);
                total_error += HALF_BLOCK_MASKS
                    .iter()
                    .map(|mask| {
                        let cell = if *mask == 0 {
                            QuarterblockCell {
                                fg: super::average_color(&quadrants, FULL_MASK),
                                bg: super::average_color(&quadrants, FULL_MASK),
                                char: ' ',
                            }
                        } else if *mask == FULL_MASK {
                            QuarterblockCell {
                                fg: super::average_color(&quadrants, FULL_MASK),
                                bg: super::average_color(&quadrants, FULL_MASK),
                                char: '█',
                            }
                        } else {
                            let (fg, bg) = super::palette_for_mask(&quadrants, *mask);
                            QuarterblockCell {
                                fg,
                                bg,
                                char: mask_to_char(*mask),
                            }
                        };
                        approximation_error(&quadrants, *mask, cell.fg, cell.bg)
                    })
                    .min()
                    .expect("halfblock masks should exist");
            }
        }

        total_error
    }

    fn char_to_mask(char: char) -> u8 {
        match char {
            ' ' => 0,
            '▘' => TOP_LEFT,
            '▝' => TOP_RIGHT,
            '▀' => TOP_LEFT | TOP_RIGHT,
            '▖' => BOTTOM_LEFT,
            '▌' => TOP_LEFT | BOTTOM_LEFT,
            '▞' => TOP_RIGHT | BOTTOM_LEFT,
            '▛' => TOP_LEFT | TOP_RIGHT | BOTTOM_LEFT,
            '▗' => BOTTOM_RIGHT,
            '▚' => TOP_LEFT | BOTTOM_RIGHT,
            '▐' => TOP_RIGHT | BOTTOM_RIGHT,
            '▜' => TOP_LEFT | TOP_RIGHT | BOTTOM_RIGHT,
            '▄' => BOTTOM_LEFT | BOTTOM_RIGHT,
            '▙' => TOP_LEFT | BOTTOM_LEFT | BOTTOM_RIGHT,
            '▟' => TOP_RIGHT | BOTTOM_LEFT | BOTTOM_RIGHT,
            '█' => FULL_MASK,
            _ => 0,
        }
    }

    #[test]
    fn encodes_all_quadrant_masks_exactly() {
        let fg = [240, 90, 210];
        let bg = [12, 18, 28];

        for mask in [0, FULL_MASK, 5, 10, 6, 9, 3, 12] {
            let quadrants = exact_mask_quadrants(mask, fg, bg);
            let cell = QuarterblockCell::new(quadrants);
            let rendered_mask = char_to_mask(cell.char);

            assert_eq!(
                approximation_error(&quadrants, rendered_mask, cell.fg, cell.bg),
                0,
                "quarterblock cell should exactly reconstruct binary quadrant masks"
            );
        }
    }

    #[test]
    fn rejects_invalid_frame_lengths() {
        let frame = CanvasFrame {
            frame_number: 1,
            timestamp_ms: 0,
            width: 2,
            height: 2,
            pixels: bytes::Bytes::from(vec![0; 3]),
        };

        let error = QuarterblocksFrame::new(&frame, Rect::new(0, 0, 1, 1))
            .expect_err("invalid frame should be rejected");
        assert_eq!(error, "invalid preview frame length");
    }

    #[test]
    fn reduces_error_for_horizontal_gradients_vs_halfblocks() {
        let frame = horizontal_gradient_frame(24, 12);
        let area = Rect::new(0, 0, 11, 4);

        assert!(
            total_quarterblock_error(&frame, area) < total_halfblock_error(&frame, area),
            "quarterblocks should preserve left-right color motion better than halfblocks"
        );
    }

    #[test]
    fn never_emits_corner_or_three_quarter_masks_for_soft_content() {
        let frame = soft_circle_frame(24, 24);
        let area = Rect::new(0, 0, 10, 6);

        for mask in used_masks(&frame, area) {
            assert!(![1, 2, 4, 7, 8, 11, 13, 14].contains(&mask));
        }
    }

    #[test]
    fn soft_circle_edges_use_nontrivial_line_safe_masks() {
        let frame = soft_circle_frame(24, 24);
        let area = Rect::new(0, 0, 10, 6);
        let masks = used_masks(&frame, area);

        assert!(
            masks
                .iter()
                .any(|mask| matches!(*mask, 5 | 10 | 6 | 9 | 3 | 12))
        );
    }

    #[test]
    fn clear_horizontal_split_uses_horizontal_mask() {
        let quadrants = [[240, 90, 210], [240, 90, 210], [12, 18, 28], [12, 18, 28]];
        let cell = QuarterblockCell::new(quadrants);

        assert_eq!(char_to_mask(cell.char), TOP_LEFT | TOP_RIGHT);
        assert_eq!(orientation_penalty(&quadrants, TOP_LEFT | TOP_RIGHT), 0);
    }
}
