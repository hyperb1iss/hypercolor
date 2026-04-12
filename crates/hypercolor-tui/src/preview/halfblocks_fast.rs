use image::Rgb;
use image::flat::{FlatSamples, SampleLayout};
use image::imageops::{FilterType, resize};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::state::CanvasFrame;

const HALF_UPPER: char = '▀';
const HALF_LOWER: char = '▄';
const SPACE: char = ' ';

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HalfblockCell {
    fg: [u8; 3],
    bg: [u8; 3],
    char: char,
}

impl HalfblockCell {
    fn new(upper: [u8; 3], lower: [u8; 3]) -> Self {
        if upper == lower {
            return Self {
                fg: upper,
                bg: lower,
                char: SPACE,
            };
        }

        if luminance(lower) > luminance(upper) {
            Self {
                fg: lower,
                bg: upper,
                char: HALF_LOWER,
            }
        } else {
            Self {
                fg: upper,
                bg: lower,
                char: HALF_UPPER,
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct HalfblocksFrame {
    rect: Rect,
    cells: Vec<HalfblockCell>,
}

impl HalfblocksFrame {
    pub(crate) fn new(frame: &CanvasFrame, area: Rect) -> Result<Self, String> {
        if area.width == 0 || area.height == 0 {
            return Ok(Self {
                rect: area,
                cells: Vec::new(),
            });
        }

        let frame_width = usize::from(frame.width);
        let frame_height = usize::from(frame.height);
        let expected_len = frame_width
            .checked_mul(frame_height)
            .and_then(|pixels| pixels.checked_mul(3))
            .ok_or_else(|| "preview dimensions overflow".to_string())?;

        if frame.pixels.len() != expected_len {
            return Err("invalid preview frame length".to_string());
        }

        let target_width = usize::from(area.width);
        let target_height = usize::from(area.height);
        let sampled_height = target_height * 2;
        let source = FlatSamples {
            samples: frame.pixels.as_ref(),
            layout: SampleLayout::row_major_packed(3, u32::from(frame.width), u32::from(frame.height)),
            color_hint: None,
        };
        let view = source
            .as_view::<Rgb<u8>>()
            .map_err(|error| format!("invalid preview frame layout: {error}"))?;
        let sampled = resize(
            &view,
            u32::from(area.width),
            u32::try_from(sampled_height).map_err(|_| "preview area too large".to_string())?,
            FilterType::Triangle,
        );
        let mut cells = Vec::with_capacity(target_width * target_height);

        for y in 0..target_height {
            for x in 0..target_width {
                cells.push(HalfblockCell::new(
                    sampled.get_pixel(
                        u32::try_from(x).expect("x fits in u32"),
                        u32::try_from(y * 2).expect("y fits in u32"),
                    )
                    .0,
                    sampled.get_pixel(
                        u32::try_from(x).expect("x fits in u32"),
                        u32::try_from(y * 2 + 1).expect("y fits in u32"),
                    )
                    .0,
                ));
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

fn luminance([r, g, b]: [u8; 3]) -> u32 {
    2126 * u32::from(r) + 7152 * u32::from(g) + 722 * u32::from(b)
}

#[cfg(test)]
mod tests {
    use super::HalfblocksFrame;
    use crate::state::CanvasFrame;
    use image::DynamicImage;
    use image::RgbImage;
    use image::imageops::FilterType;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    fn reference_cells(frame: &CanvasFrame, area: Rect) -> Vec<([u8; 3], [u8; 3], char)> {
        let image = RgbImage::from_raw(u32::from(frame.width), u32::from(frame.height), frame.pixels.to_vec())
            .expect("reference image should build");
        let resized = DynamicImage::ImageRgb8(image)
            .resize_exact(u32::from(area.width), u32::from(area.height) * 2, FilterType::Triangle)
            .to_rgb8();
        let mut cells = Vec::with_capacity(usize::from(area.width) * usize::from(area.height));

        for y in 0..u32::from(area.height) {
            for x in 0..u32::from(area.width) {
                let cell = super::HalfblockCell::new(
                    resized.get_pixel(x, y * 2).0,
                    resized.get_pixel(x, y * 2 + 1).0,
                );
                cells.push((cell.fg, cell.bg, cell.char));
            }
        }

        cells
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
    #[test]
    fn samples_top_and_bottom_colors_into_one_cell() {
        let frame = CanvasFrame {
            frame_number: 1,
            timestamp_ms: 0,
            width: 1,
            height: 2,
            pixels: bytes::Bytes::from(vec![255, 0, 0, 0, 0, 255]),
        };

        let halfblocks =
            HalfblocksFrame::new(&frame, Rect::new(0, 0, 1, 1)).expect("halfblocks should build");
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        halfblocks.render(Rect::new(0, 0, 1, 1), &mut buf);

        assert_eq!(buf[(0, 0)].symbol(), "▀");
        assert_eq!(buf[(0, 0)].fg, Color::Rgb(255, 0, 0));
        assert_eq!(buf[(0, 0)].bg, Color::Rgb(0, 0, 255));
    }

    #[test]
    fn picks_lower_block_when_lower_half_is_brighter() {
        let frame = CanvasFrame {
            frame_number: 1,
            timestamp_ms: 0,
            width: 1,
            height: 2,
            pixels: bytes::Bytes::from(vec![0, 0, 64, 255, 255, 255]),
        };

        let halfblocks =
            HalfblocksFrame::new(&frame, Rect::new(0, 0, 1, 1)).expect("halfblocks should build");
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        halfblocks.render(Rect::new(0, 0, 1, 1), &mut buf);

        assert_eq!(buf[(0, 0)].symbol(), "▄");
        assert_eq!(buf[(0, 0)].fg, Color::Rgb(255, 255, 255));
        assert_eq!(buf[(0, 0)].bg, Color::Rgb(0, 0, 64));
    }

    #[test]
    fn uses_space_when_both_halves_match() {
        let frame = CanvasFrame {
            frame_number: 1,
            timestamp_ms: 0,
            width: 1,
            height: 2,
            pixels: bytes::Bytes::from(vec![12, 34, 56, 12, 34, 56]),
        };

        let halfblocks =
            HalfblocksFrame::new(&frame, Rect::new(0, 0, 1, 1)).expect("halfblocks should build");
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        halfblocks.render(Rect::new(0, 0, 1, 1), &mut buf);

        assert_eq!(buf[(0, 0)].symbol(), " ");
        assert_eq!(buf[(0, 0)].fg, Color::Rgb(12, 34, 56));
        assert_eq!(buf[(0, 0)].bg, Color::Rgb(12, 34, 56));
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

        let error = HalfblocksFrame::new(&frame, Rect::new(0, 0, 1, 1))
            .expect_err("invalid frame should be rejected");
        assert_eq!(error, "invalid preview frame length");
    }

    #[test]
    fn matches_triangle_resize_reference_for_gradient() {
        let pixels = bytes::Bytes::from(
            (0u8..48)
                .flat_map(|n| [n, n.saturating_mul(2), 255u8.saturating_sub(n)])
                .take(6 * 4 * 3)
                .collect::<Vec<_>>(),
        );
        let frame = CanvasFrame {
            frame_number: 1,
            timestamp_ms: 0,
            width: 6,
            height: 4,
            pixels,
        };
        let area = Rect::new(0, 0, 4, 2);

        let actual = HalfblocksFrame::new(&frame, area).expect("halfblocks should build");
        let expected = reference_cells(&frame, area);

        assert_eq!(actual.cells.len(), expected.len());
        for (cell, (fg, bg, char)) in actual.cells.iter().zip(expected.iter()) {
            assert_eq!((cell.fg, cell.bg, cell.char), (*fg, *bg, *char));
        }
    }

    #[test]
    fn matches_triangle_resize_reference_for_soft_circle_edges() {
        let frame = soft_circle_frame(16, 16);
        let area = Rect::new(0, 0, 9, 5);

        let actual = HalfblocksFrame::new(&frame, area).expect("halfblocks should build");
        let expected = reference_cells(&frame, area);

        assert_eq!(actual.cells.len(), expected.len());
        for (cell, (fg, bg, char)) in actual.cells.iter().zip(expected.iter()) {
            assert_eq!((cell.fg, cell.bg, cell.char), (*fg, *bg, *char));
        }
    }
}
