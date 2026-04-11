use std::f32::consts::{FRAC_PI_2, TAU};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use cosmic_text::{FontSystem, SwashCache};
use time::{OffsetDateTime, UtcOffset};
use tiny_skia::{FillRule, LineCap, PathBuilder, Pixmap, Stroke, Transform};

use hypercolor_types::overlay::{ClockConfig, ClockStyle, HourFormat};

use super::common::{
    OverlayColor, draw_text_line, paint_from_color, parse_hex_color, render_svg_template,
    resolve_template_path,
};
use super::{OverlayBuffer, OverlayError, OverlayInput, OverlayRenderer, OverlaySize};

pub struct ClockRenderer {
    config: ClockConfig,
    primary_color: OverlayColor,
    secondary_color: OverlayColor,
    font_system: FontSystem,
    swash_cache: SwashCache,
    target_size: OverlaySize,
    template_path: Option<PathBuf>,
    template_buffer: Option<OverlayBuffer>,
    last_signature: Option<ClockSignature>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClockSignature {
    bucket: u64,
    time_text: Option<String>,
    date_text: Option<String>,
}

impl ClockRenderer {
    pub fn new(config: ClockConfig) -> Result<Self> {
        let primary_color = parse_hex_color(&config.color, "clock overlay")?;
        let secondary_color = config
            .secondary_color
            .as_deref()
            .map(|color| parse_hex_color(color, "clock overlay"))
            .transpose()?
            .unwrap_or_else(|| primary_color.with_alpha(180));
        let template_path = config
            .template
            .as_deref()
            .map(|path| resolve_template_path(Path::new(path), "clock overlay template"))
            .transpose()
            .with_context(|| "failed to resolve clock overlay template")?;

        Ok(Self {
            config,
            primary_color,
            secondary_color,
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            target_size: OverlaySize::new(1, 1),
            template_path,
            template_buffer: None,
            last_signature: None,
        })
    }

    fn reload(&mut self, target_size: OverlaySize) -> Result<()> {
        self.target_size = target_size;
        self.template_buffer = self
            .template_path
            .as_deref()
            .map(|path| render_svg_template(path, target_size, "clock overlay"))
            .transpose()?;
        self.last_signature = None;
        Ok(())
    }

    fn render_signature(&self, input: &OverlayInput<'_>) -> ClockSignature {
        let datetime = local_datetime(input.now);
        ClockSignature {
            bucket: render_bucket(input.now, &self.config),
            time_text: matches!(self.config.style, ClockStyle::Digital)
                .then(|| format_time_text(datetime, &self.config)),
            date_text: self
                .config
                .show_date
                .then(|| format_date_text(datetime, self.config.date_format.as_deref())),
        }
    }

    fn render_digital(&mut self, pixmap: &mut Pixmap, datetime: OffsetDateTime) -> Result<()> {
        let time_text = format_time_text(datetime, &self.config);
        let date_text = self
            .config
            .show_date
            .then(|| format_date_text(datetime, self.config.date_format.as_deref()));
        let target_width = self.target_size.width.max(1) as f32;
        let target_height = self.target_size.height.max(1) as f32;
        let time_area_height = if date_text.is_some() {
            target_height * 0.7
        } else {
            target_height
        };
        let date_area_height = (target_height - time_area_height).max(0.0);
        let time_font_size = digital_font_size(target_width, time_area_height, &time_text, 0.86);

        draw_text_line(
            pixmap,
            &mut self.font_system,
            &mut self.swash_cache,
            &time_text,
            self.config.font_family.as_deref(),
            time_font_size,
            self.primary_color,
            0.0,
            time_area_height,
        )?;

        if let Some(date_text) = date_text {
            let date_font_size = digital_font_size(
                target_width,
                date_area_height.max(target_height * 0.2),
                &date_text,
                0.7,
            );
            draw_text_line(
                pixmap,
                &mut self.font_system,
                &mut self.swash_cache,
                &date_text,
                self.config.font_family.as_deref(),
                date_font_size,
                self.secondary_color,
                time_area_height,
                date_area_height.max(target_height * 0.2),
            )?;
        }

        Ok(())
    }

    fn render_analog(&mut self, pixmap: &mut Pixmap, datetime: OffsetDateTime) -> Result<()> {
        let width = self.target_size.width.max(1) as f32;
        let height = self.target_size.height.max(1) as f32;
        let diameter = width.min(height);
        let radius = diameter * 0.44;
        let center_x = width / 2.0;
        let center_y = height / 2.0;

        if self.template_buffer.is_none() {
            draw_default_dial(
                pixmap,
                center_x,
                center_y,
                radius,
                self.primary_color,
                self.secondary_color,
            )?;
        }

        let second_progress =
            f32::from(datetime.second()) + datetime.nanosecond() as f32 / 1_000_000_000.0;
        let minute_progress = f32::from(datetime.minute()) + second_progress / 60.0;
        let hour_progress = f32::from((datetime.hour() % 12) as u8) + minute_progress / 60.0;

        draw_hand(
            pixmap,
            center_x,
            center_y,
            clock_angle(hour_progress / 12.0),
            radius * 0.52,
            (radius * 0.11).max(2.0),
            self.primary_color,
            radius * 0.08,
        )?;
        draw_hand(
            pixmap,
            center_x,
            center_y,
            clock_angle(minute_progress / 60.0),
            radius * 0.76,
            (radius * 0.075).max(1.5),
            self.primary_color,
            radius * 0.12,
        )?;
        if self.config.show_seconds {
            draw_hand(
                pixmap,
                center_x,
                center_y,
                clock_angle(second_progress / 60.0),
                radius * 0.84,
                (radius * 0.028).max(1.0),
                self.secondary_color,
                radius * 0.16,
            )?;
        }

        fill_circle(
            pixmap,
            center_x,
            center_y,
            (radius * 0.08).max(2.0),
            self.primary_color,
        )?;

        if self.config.show_date {
            let date_text = format_date_text(datetime, self.config.date_format.as_deref());
            let date_font_size = digital_font_size(width * 0.48, height * 0.16, &date_text, 0.72);
            let top = center_y + radius * 0.22;
            let available_height = (height - top).max(height * 0.14);
            draw_text_line(
                pixmap,
                &mut self.font_system,
                &mut self.swash_cache,
                &date_text,
                self.config.font_family.as_deref(),
                date_font_size,
                self.secondary_color,
                top,
                available_height,
            )?;
        }

        Ok(())
    }
}

impl OverlayRenderer for ClockRenderer {
    fn init(&mut self, target_size: OverlaySize) -> Result<()> {
        self.reload(target_size)
    }

    fn resize(&mut self, target_size: OverlaySize) -> Result<()> {
        self.reload(target_size)
    }

    fn render_into(
        &mut self,
        input: &OverlayInput<'_>,
        target: &mut OverlayBuffer,
    ) -> std::result::Result<(), OverlayError> {
        if target.width != self.target_size.width || target.height != self.target_size.height {
            return Err(OverlayError::Fatal(format!(
                "clock overlay target mismatch: renderer prepared {}x{}, target was {}x{}",
                self.target_size.width, self.target_size.height, target.width, target.height
            )));
        }

        let mut pixmap =
            Pixmap::new(target.width.max(1), target.height.max(1)).ok_or_else(|| {
                OverlayError::Fatal("failed to allocate clock overlay pixmap".to_owned())
            })?;
        if let Some(background) = &self.template_buffer {
            pixmap.data_mut().copy_from_slice(&background.pixels);
        }

        let datetime = local_datetime(input.now);
        match self.config.style {
            ClockStyle::Digital => self
                .render_digital(&mut pixmap, datetime)
                .map_err(|error| OverlayError::Fatal(error.to_string()))?,
            ClockStyle::Analog => self
                .render_analog(&mut pixmap, datetime)
                .map_err(|error| OverlayError::Fatal(error.to_string()))?,
        }

        target
            .copy_from_pixmap(&pixmap)
            .map_err(|error| OverlayError::Fatal(error.to_string()))?;
        self.last_signature = Some(self.render_signature(input));
        Ok(())
    }

    fn content_changed(&self, input: &OverlayInput<'_>) -> bool {
        self.last_signature
            .as_ref()
            .is_none_or(|last| last != &self.render_signature(input))
    }

    fn next_refresh_after(&self) -> Option<Duration> {
        None
    }
}

fn render_bucket(now: SystemTime, config: &ClockConfig) -> u64 {
    let elapsed = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    match config.style {
        ClockStyle::Analog if config.show_seconds => {
            let millis = elapsed.as_millis();
            u64::try_from(millis / 500).unwrap_or(u64::MAX)
        }
        _ if config.show_seconds => elapsed.as_secs(),
        _ => elapsed.as_secs() / 60,
    }
}

fn local_datetime(now: SystemTime) -> OffsetDateTime {
    let utc = OffsetDateTime::from(now);
    UtcOffset::local_offset_at(utc).map_or(utc, |offset| utc.to_offset(offset))
}

fn format_time_text(datetime: OffsetDateTime, config: &ClockConfig) -> String {
    let hour = datetime.hour();
    let minute = datetime.minute();
    let second = datetime.second();
    match (config.hour_format, config.show_seconds) {
        (HourFormat::TwentyFour, true) => format!("{hour:02}:{minute:02}:{second:02}"),
        (HourFormat::TwentyFour, false) => format!("{hour:02}:{minute:02}"),
        (HourFormat::Twelve, true) => format!(
            "{}:{minute:02}:{second:02} {}",
            twelve_hour(hour),
            meridiem(hour)
        ),
        (HourFormat::Twelve, false) => {
            format!("{}:{minute:02} {}", twelve_hour(hour), meridiem(hour))
        }
    }
}

fn twelve_hour(hour: u8) -> u8 {
    match hour % 12 {
        0 => 12,
        other => other,
    }
}

fn meridiem(hour: u8) -> &'static str {
    if hour < 12 { "AM" } else { "PM" }
}

fn format_date_text(datetime: OffsetDateTime, format: Option<&str>) -> String {
    let format = format.unwrap_or("%Y-%m-%d");
    let mut rendered = String::with_capacity(format.len() + 12);
    let mut chars = format.chars();

    while let Some(ch) = chars.next() {
        if ch != '%' {
            rendered.push(ch);
            continue;
        }

        match chars.next() {
            Some('%') => rendered.push('%'),
            Some('Y') => {
                let _ = write!(rendered, "{:04}", datetime.year());
            }
            Some('y') => {
                let _ = write!(rendered, "{:02}", datetime.year().rem_euclid(100));
            }
            Some('m') => {
                let _ = write!(rendered, "{:02}", u8::from(datetime.month()));
            }
            Some('d') => {
                let _ = write!(rendered, "{:02}", datetime.day());
            }
            Some('e') => {
                let _ = write!(rendered, "{:>2}", datetime.day());
            }
            Some('H') => {
                let _ = write!(rendered, "{:02}", datetime.hour());
            }
            Some('I') => {
                let _ = write!(rendered, "{:02}", twelve_hour(datetime.hour()));
            }
            Some('M') => {
                let _ = write!(rendered, "{:02}", datetime.minute());
            }
            Some('S') => {
                let _ = write!(rendered, "{:02}", datetime.second());
            }
            Some('p') => rendered.push_str(meridiem(datetime.hour())),
            Some('a') => rendered.push_str(short_weekday(datetime.weekday())),
            Some('A') => rendered.push_str(long_weekday(datetime.weekday())),
            Some('b') => rendered.push_str(short_month(datetime.month())),
            Some('B') => rendered.push_str(long_month(datetime.month())),
            Some(other) => {
                rendered.push('%');
                rendered.push(other);
            }
            None => rendered.push('%'),
        }
    }

    rendered
}

fn short_weekday(weekday: time::Weekday) -> &'static str {
    match weekday {
        time::Weekday::Monday => "Mon",
        time::Weekday::Tuesday => "Tue",
        time::Weekday::Wednesday => "Wed",
        time::Weekday::Thursday => "Thu",
        time::Weekday::Friday => "Fri",
        time::Weekday::Saturday => "Sat",
        time::Weekday::Sunday => "Sun",
    }
}

fn long_weekday(weekday: time::Weekday) -> &'static str {
    match weekday {
        time::Weekday::Monday => "Monday",
        time::Weekday::Tuesday => "Tuesday",
        time::Weekday::Wednesday => "Wednesday",
        time::Weekday::Thursday => "Thursday",
        time::Weekday::Friday => "Friday",
        time::Weekday::Saturday => "Saturday",
        time::Weekday::Sunday => "Sunday",
    }
}

fn short_month(month: time::Month) -> &'static str {
    match month {
        time::Month::January => "Jan",
        time::Month::February => "Feb",
        time::Month::March => "Mar",
        time::Month::April => "Apr",
        time::Month::May => "May",
        time::Month::June => "Jun",
        time::Month::July => "Jul",
        time::Month::August => "Aug",
        time::Month::September => "Sep",
        time::Month::October => "Oct",
        time::Month::November => "Nov",
        time::Month::December => "Dec",
    }
}

fn long_month(month: time::Month) -> &'static str {
    match month {
        time::Month::January => "January",
        time::Month::February => "February",
        time::Month::March => "March",
        time::Month::April => "April",
        time::Month::May => "May",
        time::Month::June => "June",
        time::Month::July => "July",
        time::Month::August => "August",
        time::Month::September => "September",
        time::Month::October => "October",
        time::Month::November => "November",
        time::Month::December => "December",
    }
}

fn digital_font_size(width: f32, height: f32, text: &str, height_ratio: f32) -> f32 {
    let char_count = text.chars().count().max(1) as f32;
    let width_limited = width / char_count * 1.65;
    width_limited.min(height * height_ratio).max(1.0)
}

fn draw_default_dial(
    pixmap: &mut Pixmap,
    center_x: f32,
    center_y: f32,
    radius: f32,
    primary: OverlayColor,
    secondary: OverlayColor,
) -> Result<()> {
    let ring = PathBuilder::from_circle(center_x, center_y, radius)
        .ok_or_else(|| anyhow!("failed to build analog clock ring"))?;
    let mut ring_stroke = Stroke::default();
    ring_stroke.width = (radius * 0.06).max(1.5);
    ring_stroke.line_cap = LineCap::Round;
    stroke_path(pixmap, &ring, secondary, &ring_stroke);

    for index in 0..12 {
        let angle = clock_angle(index as f32 / 12.0);
        let outer = polar_point(center_x, center_y, radius * 0.84, angle);
        let inner_radius = if index % 3 == 0 {
            radius * 0.60
        } else {
            radius * 0.68
        };
        let inner = polar_point(center_x, center_y, inner_radius, angle);
        let path = line_path(inner, outer)?;
        let mut stroke = Stroke::default();
        stroke.width = if index % 3 == 0 {
            (radius * 0.055).max(1.5)
        } else {
            (radius * 0.03).max(1.0)
        };
        stroke.line_cap = LineCap::Round;
        stroke_path(pixmap, &path, primary, &stroke);
    }

    Ok(())
}

fn draw_hand(
    pixmap: &mut Pixmap,
    center_x: f32,
    center_y: f32,
    angle: f32,
    length: f32,
    width: f32,
    color: OverlayColor,
    tail_length: f32,
) -> Result<()> {
    let tail = polar_point(
        center_x,
        center_y,
        tail_length,
        angle + std::f32::consts::PI,
    );
    let head = polar_point(center_x, center_y, length, angle);
    let path = line_path(tail, head)?;
    let mut stroke = Stroke::default();
    stroke.width = width.max(1.0);
    stroke.line_cap = LineCap::Round;
    stroke_path(pixmap, &path, color, &stroke);
    Ok(())
}

fn fill_circle(
    pixmap: &mut Pixmap,
    center_x: f32,
    center_y: f32,
    radius: f32,
    color: OverlayColor,
) -> Result<()> {
    let path = PathBuilder::from_circle(center_x, center_y, radius)
        .ok_or_else(|| anyhow!("failed to build analog clock center"))?;
    let paint = paint_from_color(color);
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    Ok(())
}

fn line_path(start: (f32, f32), end: (f32, f32)) -> Result<tiny_skia::Path> {
    let mut builder = PathBuilder::new();
    builder.move_to(start.0, start.1);
    builder.line_to(end.0, end.1);
    builder
        .finish()
        .ok_or_else(|| anyhow!("failed to build analog clock hand path"))
}

fn stroke_path(pixmap: &mut Pixmap, path: &tiny_skia::Path, color: OverlayColor, stroke: &Stroke) {
    let paint = paint_from_color(color);
    pixmap.stroke_path(path, &paint, stroke, Transform::identity(), None);
}

fn polar_point(center_x: f32, center_y: f32, distance: f32, angle: f32) -> (f32, f32) {
    (
        center_x + distance * angle.cos(),
        center_y + distance * angle.sin(),
    )
}

fn clock_angle(progress: f32) -> f32 {
    progress * TAU - FRAC_PI_2
}
