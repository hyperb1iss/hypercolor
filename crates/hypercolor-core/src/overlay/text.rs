use anyhow::{Result, anyhow, bail};
use cosmic_text::{
    Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap,
};
use tiny_skia::{ColorU8, Pixmap, PremultipliedColorU8};

use hypercolor_types::overlay::{TextAlign, TextOverlayConfig};

use super::{
    OverlayBuffer, OverlayError, OverlayInput, OverlayRenderer, OverlaySize,
    overlay_buffer_from_pixmap,
};

const SENSOR_PREFIX: &str = "{sensor:";
const SCROLL_GAP_MIN: f32 = 12.0;
const LINE_HEIGHT_SCALE: f32 = 1.2;

pub struct TextRenderer {
    config: TextOverlayConfig,
    text_color: Color,
    font_system: FontSystem,
    swash_cache: SwashCache,
    target_size: OverlaySize,
    last_resolved_text: Option<String>,
    last_scroll_step: Option<i32>,
    last_scroll_cycle_width: Option<f32>,
}

impl TextRenderer {
    pub fn new(config: TextOverlayConfig) -> Result<Self> {
        let text_color = parse_text_color(&config.color)?;
        Ok(Self {
            config,
            text_color,
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            target_size: OverlaySize::new(1, 1),
            last_resolved_text: None,
            last_scroll_step: None,
            last_scroll_cycle_width: None,
        })
    }

    fn render_state(&self, input: &OverlayInput<'_>) -> RenderState {
        let resolved_text = interpolate_sensor_tokens(&self.config.text, input);
        let scroll_step = self.last_scroll_cycle_width.and_then(|cycle_width| {
            current_scroll_step(
                input.elapsed_secs,
                cycle_width,
                self.target_size.width,
                self.config.scroll_speed,
            )
        });

        RenderState {
            resolved_text,
            scroll_step,
        }
    }
}

impl OverlayRenderer for TextRenderer {
    fn init(&mut self, target_size: OverlaySize) -> Result<()> {
        self.target_size = target_size;
        self.last_resolved_text = None;
        self.last_scroll_step = None;
        self.last_scroll_cycle_width = None;
        Ok(())
    }

    fn resize(&mut self, target_size: OverlaySize) -> Result<()> {
        self.init(target_size)
    }

    fn render_into(
        &mut self,
        input: &OverlayInput<'_>,
        target: &mut OverlayBuffer,
    ) -> std::result::Result<(), OverlayError> {
        if target.width != self.target_size.width || target.height != self.target_size.height {
            return Err(OverlayError::Fatal(format!(
                "text overlay target mismatch: renderer prepared {}x{}, target was {}x{}",
                self.target_size.width, self.target_size.height, target.width, target.height
            )));
        }

        let state = self.render_state(input);
        let target_width = self.target_size.width.max(1);
        let target_height = self.target_size.height.max(1);
        let mut pixmap = Pixmap::new(target_width, target_height).ok_or_else(|| {
            OverlayError::Fatal("failed to allocate text overlay pixmap".to_owned())
        })?;

        let metrics = Metrics::relative(self.config.font_size.max(1.0), LINE_HEIGHT_SCALE);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        let scroll_enabled = self.config.scroll;
        let width_opt = if scroll_enabled {
            None
        } else {
            Some(target_width as f32)
        };
        buffer.set_wrap(
            &mut self.font_system,
            if scroll_enabled {
                Wrap::None
            } else {
                Wrap::WordOrGlyph
            },
        );
        buffer.set_size(&mut self.font_system, width_opt, Some(target_height as f32));
        buffer.set_text(
            &mut self.font_system,
            &state.resolved_text,
            &text_attrs(&self.config, self.text_color),
            Shaping::Advanced,
            (!scroll_enabled).then_some(text_alignment(self.config.align)),
        );

        let layout = measure_buffer(&buffer);
        let vertical_offset =
            ((target_height as f32 - layout.content_height).max(0.0) / 2.0).round();
        let scroll_cycle_width = if scroll_enabled && layout.max_line_width > target_width as f32 {
            Some(layout.max_line_width + scroll_gap(self.config.font_size) + target_width as f32)
        } else {
            None
        };
        let horizontal_offset = match scroll_cycle_width {
            Some(cycle_width) => {
                target_width as f32
                    - scroll_distance(input.elapsed_secs, cycle_width, self.config.scroll_speed)
            }
            None => static_alignment_offset(self.config.align, target_width, layout.max_line_width),
        };

        render_text_buffer(
            &mut pixmap,
            &mut buffer,
            &mut self.font_system,
            &mut self.swash_cache,
            self.text_color,
            horizontal_offset.round() as i32,
            vertical_offset.round() as i32,
        );

        let premul = overlay_buffer_from_pixmap(&pixmap)
            .map_err(|error| OverlayError::Fatal(error.to_string()))?;
        target.pixels.copy_from_slice(&premul.pixels);
        self.last_resolved_text = Some(state.resolved_text);
        self.last_scroll_step = scroll_cycle_width.and_then(|cycle_width| {
            current_scroll_step(
                input.elapsed_secs,
                cycle_width,
                target_width,
                self.config.scroll_speed,
            )
        });
        self.last_scroll_cycle_width = scroll_cycle_width;
        Ok(())
    }

    fn content_changed(&self, input: &OverlayInput<'_>) -> bool {
        if !self.config.scroll {
            return false;
        }

        let state = self.render_state(input);
        self.last_resolved_text
            .as_deref()
            .is_none_or(|last| last != state.resolved_text)
            || self.last_scroll_step != state.scroll_step
    }
}

struct RenderState {
    resolved_text: String,
    scroll_step: Option<i32>,
}

struct BufferLayout {
    max_line_width: f32,
    content_height: f32,
}

fn text_attrs(config: &TextOverlayConfig, color: Color) -> Attrs<'_> {
    let attrs = Attrs::new().color(color);
    match config.font_family.as_deref() {
        Some(family) if !family.trim().is_empty() => attrs.family(Family::Name(family)),
        _ => attrs,
    }
}

fn text_alignment(align: TextAlign) -> Align {
    match align {
        TextAlign::Left => Align::Left,
        TextAlign::Center => Align::Center,
        TextAlign::Right => Align::Right,
    }
}

fn measure_buffer(buffer: &Buffer) -> BufferLayout {
    let mut max_line_width = 0.0_f32;
    let mut content_height = 0.0_f32;
    for run in buffer.layout_runs() {
        max_line_width = max_line_width.max(run.line_w);
        content_height = content_height.max(run.line_top + run.line_height);
    }

    BufferLayout {
        max_line_width,
        content_height,
    }
}

fn render_text_buffer(
    pixmap: &mut Pixmap,
    buffer: &mut Buffer,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    color: Color,
    offset_x: i32,
    offset_y: i32,
) {
    let width = pixmap.width();
    let height = pixmap.height();
    let pixels = pixmap.pixels_mut();
    buffer.draw(
        font_system,
        swash_cache,
        color,
        |x, y, w, h, pixel_color| {
            for draw_y in 0..h {
                for draw_x in 0..w {
                    let px = x
                        .saturating_add(offset_x)
                        .saturating_add(i32::try_from(draw_x).unwrap_or_default());
                    let py = y
                        .saturating_add(offset_y)
                        .saturating_add(i32::try_from(draw_y).unwrap_or_default());
                    blend_text_pixel(pixels, width, height, px, py, pixel_color);
                }
            }
        },
    );
}

fn blend_text_pixel(
    pixels: &mut [PremultipliedColorU8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    color: Color,
) {
    if x < 0 || y < 0 {
        return;
    }
    let x = u32::try_from(x).unwrap_or_default();
    let y = u32::try_from(y).unwrap_or_default();
    if x >= width || y >= height {
        return;
    }

    let source = ColorU8::from_rgba(color.r(), color.g(), color.b(), color.a()).premultiply();
    if source.alpha() == 0 {
        return;
    }

    let index = usize::try_from(y)
        .unwrap_or_default()
        .saturating_mul(usize::try_from(width).unwrap_or_default())
        .saturating_add(usize::try_from(x).unwrap_or_default());
    let destination = pixels[index];
    let blended = blend_source_over(destination, source);
    pixels[index] = blended;
}

fn blend_source_over(
    destination: PremultipliedColorU8,
    source: PremultipliedColorU8,
) -> PremultipliedColorU8 {
    if source.alpha() == u8::MAX {
        return source;
    }
    if source.alpha() == 0 {
        return destination;
    }

    let inverse_alpha = u16::from(u8::MAX.saturating_sub(source.alpha()));
    let blend = |dst: u8, src: u8| -> u8 {
        let composed = u16::from(src).saturating_add(
            u16::from(dst)
                .saturating_mul(inverse_alpha)
                .saturating_add(127)
                / 255,
        );
        u8::try_from(composed.min(u16::from(u8::MAX))).unwrap_or(u8::MAX)
    };
    let alpha = blend(destination.alpha(), source.alpha());
    PremultipliedColorU8::from_rgba(
        blend(destination.red(), source.red()).min(alpha),
        blend(destination.green(), source.green()).min(alpha),
        blend(destination.blue(), source.blue()).min(alpha),
        alpha,
    )
    .expect("source-over blend should preserve premultiplied pixel invariants")
}

fn static_alignment_offset(align: TextAlign, target_width: u32, line_width: f32) -> f32 {
    let slack = (target_width as f32 - line_width).max(0.0);
    match align {
        TextAlign::Left => 0.0,
        TextAlign::Center => slack / 2.0,
        TextAlign::Right => slack,
    }
}

fn scroll_gap(font_size: f32) -> f32 {
    font_size.max(SCROLL_GAP_MIN)
}

fn current_scroll_step(
    elapsed_secs: f32,
    cycle_width: f32,
    target_width: u32,
    scroll_speed: f32,
) -> Option<i32> {
    (cycle_width > target_width as f32)
        .then(|| scroll_distance(elapsed_secs, cycle_width, scroll_speed).round() as i32)
}

fn scroll_distance(elapsed_secs: f32, cycle_width: f32, scroll_speed: f32) -> f32 {
    let speed = scroll_speed.max(1.0);
    (elapsed_secs.max(0.0) * speed).rem_euclid(cycle_width.max(1.0))
}

fn interpolate_sensor_tokens(template: &str, input: &OverlayInput<'_>) -> String {
    let mut output = String::with_capacity(template.len());
    let mut cursor = template;

    while let Some(start) = cursor.find(SENSOR_PREFIX) {
        let (before, after_prefix) = cursor.split_at(start);
        output.push_str(before);

        let after_prefix = &after_prefix[SENSOR_PREFIX.len()..];
        let Some(end) = after_prefix.find('}') else {
            output.push_str(&cursor[start..]);
            return output;
        };

        let label = after_prefix[..end].trim();
        output.push_str(&input.sensors.reading(label).map_or_else(
            || "--".to_owned(),
            |reading| format_sensor_value(reading.value),
        ));
        cursor = &after_prefix[end + 1..];
    }

    output.push_str(cursor);
    output
}

fn format_sensor_value(value: f32) -> String {
    if !value.is_finite() {
        return "--".to_owned();
    }
    if (value.fract().abs() < 0.05) || value.abs() >= 100.0 {
        return format!("{}", value.round() as i32);
    }

    let formatted = format!("{value:.1}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

fn parse_text_color(raw: &str) -> Result<Color> {
    let hex = raw.trim().trim_start_matches('#');
    let rgba = match hex.len() {
        3 => {
            let bytes = hex.as_bytes();
            [
                expanded_nibble(bytes[0])?,
                expanded_nibble(bytes[1])?,
                expanded_nibble(bytes[2])?,
                u8::MAX,
            ]
        }
        6 => [
            u8::from_str_radix(&hex[0..2], 16)
                .map_err(|_| anyhow!("invalid text overlay color '{raw}'"))?,
            u8::from_str_radix(&hex[2..4], 16)
                .map_err(|_| anyhow!("invalid text overlay color '{raw}'"))?,
            u8::from_str_radix(&hex[4..6], 16)
                .map_err(|_| anyhow!("invalid text overlay color '{raw}'"))?,
            u8::MAX,
        ],
        8 => [
            u8::from_str_radix(&hex[0..2], 16)
                .map_err(|_| anyhow!("invalid text overlay color '{raw}'"))?,
            u8::from_str_radix(&hex[2..4], 16)
                .map_err(|_| anyhow!("invalid text overlay color '{raw}'"))?,
            u8::from_str_radix(&hex[4..6], 16)
                .map_err(|_| anyhow!("invalid text overlay color '{raw}'"))?,
            u8::from_str_radix(&hex[6..8], 16)
                .map_err(|_| anyhow!("invalid text overlay color '{raw}'"))?,
        ],
        _ => bail!("unsupported text overlay color '{raw}'; expected #rgb, #rrggbb, or #rrggbbaa"),
    };

    Ok(Color::rgba(rgba[0], rgba[1], rgba[2], rgba[3]))
}

fn expanded_nibble(byte: u8) -> Result<u8> {
    let nibble = match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => bail!("invalid text overlay color nibble"),
    };
    Ok((nibble << 4) | nibble)
}
