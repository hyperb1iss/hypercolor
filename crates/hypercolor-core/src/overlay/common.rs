use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use cosmic_text::{Align, Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap};
use resvg::{tiny_skia, usvg};
use tiny_skia::{Paint, Pixmap, PremultipliedColorU8, Transform};

use super::{OverlayBuffer, OverlaySize};
use crate::config::paths::{config_dir, data_dir};

const LINE_HEIGHT_SCALE: f32 = 1.2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OverlayColor {
    red: u8,
    green: u8,
    blue: u8,
    alpha: u8,
}

impl OverlayColor {
    pub(crate) const fn rgba(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    pub(crate) const fn with_alpha(self, alpha: u8) -> Self {
        Self { alpha, ..self }
    }

    pub(crate) const fn red(self) -> u8 {
        self.red
    }

    pub(crate) const fn green(self) -> u8 {
        self.green
    }

    pub(crate) const fn blue(self) -> u8 {
        self.blue
    }

    pub(crate) const fn alpha(self) -> u8 {
        self.alpha
    }

    pub(crate) const fn text_color(self) -> cosmic_text::Color {
        cosmic_text::Color::rgba(self.red, self.green, self.blue, self.alpha)
    }

    pub(crate) fn skia_color(self) -> tiny_skia::Color {
        tiny_skia::Color::from_rgba8(self.red, self.green, self.blue, self.alpha)
    }
}

pub(crate) fn parse_hex_color(raw: &str, context: &str) -> Result<OverlayColor> {
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
                .map_err(|_| anyhow!("invalid {context} color '{raw}'"))?,
            u8::from_str_radix(&hex[2..4], 16)
                .map_err(|_| anyhow!("invalid {context} color '{raw}'"))?,
            u8::from_str_radix(&hex[4..6], 16)
                .map_err(|_| anyhow!("invalid {context} color '{raw}'"))?,
            u8::MAX,
        ],
        8 => [
            u8::from_str_radix(&hex[0..2], 16)
                .map_err(|_| anyhow!("invalid {context} color '{raw}'"))?,
            u8::from_str_radix(&hex[2..4], 16)
                .map_err(|_| anyhow!("invalid {context} color '{raw}'"))?,
            u8::from_str_radix(&hex[4..6], 16)
                .map_err(|_| anyhow!("invalid {context} color '{raw}'"))?,
            u8::from_str_radix(&hex[6..8], 16)
                .map_err(|_| anyhow!("invalid {context} color '{raw}'"))?,
        ],
        _ => {
            bail!("unsupported {context} color '{raw}'; expected #rgb, #rrggbb, or #rrggbbaa")
        }
    };

    Ok(OverlayColor::rgba(rgba[0], rgba[1], rgba[2], rgba[3]))
}

pub(crate) fn lerp_color(start: OverlayColor, end: OverlayColor, progress: f32) -> OverlayColor {
    let progress = progress.clamp(0.0, 1.0);
    let blend = |from: u8, to: u8| -> u8 {
        let value = f32::from(from) + (f32::from(to) - f32::from(from)) * progress;
        value.round().clamp(0.0, 255.0) as u8
    };

    OverlayColor::rgba(
        blend(start.red(), end.red()),
        blend(start.green(), end.green()),
        blend(start.blue(), end.blue()),
        blend(start.alpha(), end.alpha()),
    )
}

pub(crate) fn draw_text_line(
    pixmap: &mut Pixmap,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    text: &str,
    font_family: Option<&str>,
    font_size: f32,
    color: OverlayColor,
    top: f32,
    height: f32,
) -> Result<()> {
    if text.trim().is_empty() || height <= 0.0 {
        return Ok(());
    }

    let metrics = Metrics::relative(font_size.max(1.0), LINE_HEIGHT_SCALE);
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_wrap(font_system, Wrap::None);
    buffer.set_size(
        font_system,
        Some(pixmap.width() as f32),
        Some(height.max(1.0)),
    );
    buffer.set_text(
        font_system,
        text,
        &text_attrs(font_family, color.text_color()),
        Shaping::Advanced,
        Some(Align::Center),
    );

    let content_height = measure_buffer_height(&buffer);
    let vertical_offset = (top + ((height - content_height).max(0.0) / 2.0)).round() as i32;
    render_text_buffer(
        pixmap,
        &mut buffer,
        font_system,
        swash_cache,
        color,
        0,
        vertical_offset,
    );
    Ok(())
}

pub(crate) fn paint_from_color(color: OverlayColor) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.red(), color.green(), color.blue(), color.alpha());
    paint.anti_alias = true;
    paint
}

pub(crate) fn render_svg_template(
    path: &Path,
    target_size: OverlaySize,
    context: &str,
) -> Result<OverlayBuffer> {
    let data = fs::read(path)
        .with_context(|| format!("failed to read {context} template '{}'", path.display()))?;
    let mut options = usvg::Options::default();
    options.resources_dir = path.parent().map(Path::to_path_buf);
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_data(&data, &options)
        .with_context(|| format!("failed to parse {context} template '{}'", path.display()))?;

    let mut pixmap = Pixmap::new(target_size.width.max(1), target_size.height.max(1))
        .ok_or_else(|| anyhow!("failed to allocate {context} template pixmap"))?;
    let svg_size = tree.size();
    let scale = (target_size.width as f32 / svg_size.width())
        .min(target_size.height as f32 / svg_size.height());
    let dx = ((target_size.width as f32 - svg_size.width() * scale) / 2.0).max(0.0);
    let dy = ((target_size.height as f32 - svg_size.height() * scale) / 2.0).max(0.0);
    let transform = Transform::from_scale(scale, scale).post_translate(dx, dy);
    let mut pixmap_mut = pixmap.as_mut();
    resvg::render(&tree, transform, &mut pixmap_mut);

    let mut buffer = OverlayBuffer::new(target_size);
    buffer.copy_from_pixmap(&pixmap)?;
    Ok(buffer)
}

pub(crate) fn bundled_overlay_templates_root() -> PathBuf {
    let installed = data_dir().join("overlay-templates");
    if installed.is_dir() {
        return installed;
    }

    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/overlay-templates")
}

pub(crate) fn user_overlay_templates_dir() -> PathBuf {
    config_dir().join("templates")
}

pub(crate) fn resolve_template_path(path: &Path, context: &str) -> Result<PathBuf> {
    if path.is_absolute() {
        if path.exists() {
            return Ok(path.to_path_buf());
        }
        bail!("absolute {context} path does not exist: {}", path.display());
    }

    let mut candidates = vec![
        bundled_overlay_templates_root().join(path),
        user_overlay_templates_dir().join(path),
    ];
    if let Ok(current_dir) = std::env::current_dir() {
        candidates.push(current_dir.join(path));
    }
    candidates.push(path.to_path_buf());

    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    bail!(
        "could not resolve {context} '{}'; searched bundled, user, current, and raw relative paths",
        path.display()
    );
}

pub(crate) fn format_sensor_value(value: f32) -> String {
    if !value.is_finite() {
        return "--".to_owned();
    }
    if (value.fract().abs() < 0.05) || value.abs() >= 100.0 {
        return format!("{}", value.round() as i32);
    }

    format!("{value:.1}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

fn text_attrs<'a>(font_family: Option<&'a str>, color: cosmic_text::Color) -> Attrs<'a> {
    let attrs = Attrs::new().color(color);
    match font_family {
        Some(family) if !family.trim().is_empty() => attrs.family(Family::Name(family)),
        _ => attrs,
    }
}

fn measure_buffer_height(buffer: &Buffer) -> f32 {
    let mut content_height = 0.0_f32;
    for run in buffer.layout_runs() {
        content_height = content_height.max(run.line_top + run.line_height);
    }

    content_height
}

fn render_text_buffer(
    pixmap: &mut Pixmap,
    buffer: &mut Buffer,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    color: OverlayColor,
    offset_x: i32,
    offset_y: i32,
) {
    let width = pixmap.width();
    let height = pixmap.height();
    let pixels = pixmap.pixels_mut();
    buffer.draw(
        font_system,
        swash_cache,
        color.text_color(),
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
    color: cosmic_text::Color,
) {
    if x < 0 || y < 0 {
        return;
    }
    let x = u32::try_from(x).unwrap_or_default();
    let y = u32::try_from(y).unwrap_or_default();
    if x >= width || y >= height {
        return;
    }

    let source =
        tiny_skia::ColorU8::from_rgba(color.r(), color.g(), color.b(), color.a()).premultiply();
    if source.alpha() == 0 {
        return;
    }

    let index = usize::try_from(y)
        .unwrap_or_default()
        .saturating_mul(usize::try_from(width).unwrap_or_default())
        .saturating_add(usize::try_from(x).unwrap_or_default());
    let destination = pixels[index];
    pixels[index] = blend_source_over(destination, source);
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

fn expanded_nibble(byte: u8) -> Result<u8> {
    let nibble = match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => bail!("invalid overlay color nibble"),
    };
    Ok((nibble << 4) | nibble)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::tempdir;

    use super::{bundled_overlay_templates_root, resolve_template_path};

    #[test]
    fn bundled_overlay_templates_root_ends_with_overlay_templates() {
        let root = bundled_overlay_templates_root();
        assert_eq!(
            root.file_name().and_then(|value| value.to_str()),
            Some("overlay-templates")
        );
        assert!(root.exists(), "bundled overlay template root should exist");
    }

    #[test]
    fn resolve_template_path_accepts_existing_absolute() {
        let dir = tempdir().expect("tempdir should create");
        let template_path = dir.path().join("template.svg");
        std::fs::write(&template_path, "<svg/>").expect("write should work");

        let resolved = resolve_template_path(&template_path, "overlay template")
            .expect("absolute path should resolve");
        assert_eq!(resolved, template_path);
    }

    #[test]
    fn resolve_template_path_rejects_missing_file() {
        let missing = Path::new("this/path/does/not/exist.svg");
        let error = resolve_template_path(missing, "overlay template")
            .expect_err("missing path should fail");
        assert!(
            error
                .to_string()
                .contains("could not resolve overlay template")
        );
    }
}
