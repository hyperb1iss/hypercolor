use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::state::CanvasFrame;

const HALF_BLOCK: &str = "▀";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HalfblockCell {
    upper: [u8; 3],
    lower: [u8; 3],
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
        let doubled_height = target_height * 2;
        let mut cells = Vec::with_capacity(target_width * target_height);

        for y in 0..target_height {
            let upper_y = sample_index(y * 2, doubled_height, frame_height);
            let lower_y = sample_index(y * 2 + 1, doubled_height, frame_height);

            for x in 0..target_width {
                let src_x = sample_index(x, target_width, frame_width);
                cells.push(HalfblockCell {
                    upper: read_pixel(frame.pixels.as_ref(), frame_width, src_x, upper_y),
                    lower: read_pixel(frame.pixels.as_ref(), frame_width, src_x, lower_y),
                });
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
                        .set_symbol(HALF_BLOCK)
                        .set_fg(Color::Rgb(cell.upper[0], cell.upper[1], cell.upper[2]))
                        .set_bg(Color::Rgb(cell.lower[0], cell.lower[1], cell.lower[2]));
                }
            }
        }
    }
}

fn sample_index(index: usize, target_len: usize, source_len: usize) -> usize {
    (((index * 2 + 1) * source_len) / (target_len * 2)).min(source_len.saturating_sub(1))
}

fn read_pixel(pixels: &[u8], width: usize, x: usize, y: usize) -> [u8; 3] {
    let offset = (y * width + x) * 3;
    [pixels[offset], pixels[offset + 1], pixels[offset + 2]]
}

#[cfg(test)]
mod tests {
    use super::HalfblocksFrame;
    use crate::state::CanvasFrame;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
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
}
