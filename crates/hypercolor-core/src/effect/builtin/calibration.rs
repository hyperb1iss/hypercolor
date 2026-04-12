//! Calibration renderer — high-contrast diagnostic patterns for layout setup.
//!
//! Designed to answer practical setup questions:
//! - Where does a device sit on the shared canvas?
//! - Is it rotated or mirrored relative to the rest of the layout?
//! - Does the sampled footprint look correct?

use std::f32::consts::TAU;
use std::path::PathBuf;

use hypercolor_types::canvas::{BYTES_PER_PIXEL, Canvas, linear_to_srgb_u8};
use hypercolor_types::effect::{
    ControlDefinition, ControlValue, EffectCategory, EffectMetadata, EffectSource, PresetTemplate,
};

use super::common::{
    builtin_effect_id, color_control, dropdown_control, preset_with_desc, slider_control,
    toggle_control,
};
use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};

const LEAD_COLOR: [f32; 4] = [0.07, 1.00, 0.96, 1.0];
const TRAIL_COLOR: [f32; 4] = [1.00, 0.12, 0.86, 1.0];
const ACCENT_COLOR: [f32; 4] = [0.94, 0.97, 1.00, 1.0];
const BACKGROUND_COLOR: [f32; 4] = [0.01, 0.01, 0.05, 1.0];
const ORIENTATION_COLORS: [[f32; 4]; 4] = [
    LEAD_COLOR,              // top-left: laser cyan
    TRAIL_COLOR,             // top-right: hot magenta
    [1.00, 0.18, 0.30, 1.0], // bottom-right: signal red
    [0.32, 1.00, 0.10, 1.0], // bottom-left: acid green
];

type Srgba = [u8; BYTES_PER_PIXEL];

#[derive(Clone, Copy)]
struct CalibrationPalette {
    primary: Srgba,
    secondary: Srgba,
    accent: Srgba,
    background: Srgba,
    orientation: [Srgba; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CalibrationPattern {
    Sweep,
    OpposingSweeps,
    Crosshair,
    QuadrantCycle,
    CornerCycle,
    Rings,
}

impl CalibrationPattern {
    fn from_str(value: &str) -> Self {
        match normalize_choice(value).as_str() {
            "opposing_sweeps" => Self::OpposingSweeps,
            "crosshair" => Self::Crosshair,
            "quadrant_cycle" => Self::QuadrantCycle,
            "corner_cycle" => Self::CornerCycle,
            "rings" => Self::Rings,
            _ => Self::Sweep,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CalibrationDirection {
    LeftToRight,
    RightToLeft,
    TopToBottom,
    BottomToTop,
    TopLeftToBottomRight,
    BottomRightToTopLeft,
    TopRightToBottomLeft,
    BottomLeftToTopRight,
    Clockwise,
    CounterClockwise,
    Outward,
    Inward,
}

impl CalibrationDirection {
    fn from_str(value: &str) -> Self {
        match normalize_choice(value).as_str() {
            "right_to_left" => Self::RightToLeft,
            "top_to_bottom" => Self::TopToBottom,
            "bottom_to_top" => Self::BottomToTop,
            "top_left_to_bottom_right" => Self::TopLeftToBottomRight,
            "bottom_right_to_top_left" => Self::BottomRightToTopLeft,
            "top_right_to_bottom_left" => Self::TopRightToBottomLeft,
            "bottom_left_to_top_right" => Self::BottomLeftToTopRight,
            "counter_clockwise" => Self::CounterClockwise,
            "outward" => Self::Outward,
            "inward" => Self::Inward,
            "clockwise" => Self::Clockwise,
            _ => Self::LeftToRight,
        }
    }

    fn linear_position(self, nx: f32, ny: f32) -> f32 {
        match self {
            Self::LeftToRight => nx,
            Self::RightToLeft => 1.0 - nx,
            Self::TopToBottom => ny,
            Self::BottomToTop => 1.0 - ny,
            Self::TopLeftToBottomRight => (nx + ny) * 0.5,
            Self::BottomRightToTopLeft => 1.0 - ((nx + ny) * 0.5),
            Self::TopRightToBottomLeft => ((1.0 - nx) + ny) * 0.5,
            Self::BottomLeftToTopRight => (nx + (1.0 - ny)) * 0.5,
            Self::Clockwise | Self::CounterClockwise | Self::Outward | Self::Inward => nx,
        }
    }

    fn crosshair_center(self, phase: f32) -> (f32, f32) {
        match self {
            Self::LeftToRight => (phase, 0.5),
            Self::RightToLeft => (1.0 - phase, 0.5),
            Self::TopToBottom => (0.5, phase),
            Self::BottomToTop => (0.5, 1.0 - phase),
            Self::TopLeftToBottomRight => (phase, phase),
            Self::BottomRightToTopLeft => (1.0 - phase, 1.0 - phase),
            Self::TopRightToBottomLeft => (1.0 - phase, phase),
            Self::BottomLeftToTopRight => (phase, 1.0 - phase),
            Self::Clockwise | Self::CounterClockwise | Self::Outward | Self::Inward => {
                (phase, phase)
            }
        }
    }
}

/// High-contrast calibration scenes for layout placement, rotation, and coverage checks.
pub struct CalibrationRenderer {
    pattern: CalibrationPattern,
    direction: CalibrationDirection,
    primary_color: [f32; 4],
    secondary_color: [f32; 4],
    accent_color: [f32; 4],
    background_color: [f32; 4],
    speed: f32,
    size: f32,
    softness: f32,
    show_grid: bool,
    grid_scale: f32,
    brightness: f32,
}

impl CalibrationRenderer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            pattern: CalibrationPattern::Sweep,
            direction: CalibrationDirection::LeftToRight,
            primary_color: LEAD_COLOR,
            secondary_color: TRAIL_COLOR,
            accent_color: ACCENT_COLOR,
            background_color: BACKGROUND_COLOR,
            speed: 18.0,
            size: 22.0,
            softness: 18.0,
            show_grid: false,
            grid_scale: 8.0,
            brightness: 1.0,
        }
    }

    fn cycles_per_second(&self) -> f32 {
        self.speed.clamp(0.0, 100.0) * 0.006
    }

    fn phase(&self, time_secs: f32) -> f32 {
        (time_secs * self.cycles_per_second()).rem_euclid(1.0)
    }

    fn band_half_width(&self) -> f32 {
        0.02 + (self.size.clamp(1.0, 100.0) / 100.0) * 0.22
    }

    fn marker_extent(&self) -> f32 {
        0.12 + (self.size.clamp(1.0, 100.0) / 100.0) * 0.30
    }

    fn feather(&self) -> f32 {
        self.band_half_width() * (self.softness.clamp(0.0, 100.0) / 100.0)
    }

    fn scaled_srgba(&self, rgba: [f32; 4]) -> Srgba {
        [
            linear_to_srgb_u8((rgba[0] * self.brightness).clamp(0.0, 1.0)),
            linear_to_srgb_u8((rgba[1] * self.brightness).clamp(0.0, 1.0)),
            linear_to_srgb_u8((rgba[2] * self.brightness).clamp(0.0, 1.0)),
            (rgba[3] * 255.0).round().clamp(0.0, 255.0) as u8,
        ]
    }

    fn palette(&self) -> CalibrationPalette {
        CalibrationPalette {
            primary: self.scaled_srgba(self.primary_color),
            secondary: self.scaled_srgba(self.secondary_color),
            accent: self.scaled_srgba(self.accent_color),
            background: self.scaled_srgba(self.background_color),
            orientation: ORIENTATION_COLORS.map(|rgba| self.scaled_srgba(rgba)),
        }
    }

    fn sweep_color(
        &self,
        signed_distance: f32,
        intensity: f32,
        width: f32,
        palette: &CalibrationPalette,
    ) -> Srgba {
        let head_mix = ((signed_distance / width) * 0.5 + 0.5).clamp(0.0, 1.0);
        let edge = blend_srgba(palette.secondary, palette.primary, head_mix);
        let accent_mix = (intensity * intensity * 0.35).clamp(0.0, 1.0);
        blend_srgba(edge, palette.accent, accent_mix)
    }

    fn render_sweep(&self, nx: f32, ny: f32, phase: f32, palette: &CalibrationPalette) -> Srgba {
        let position = self.direction.linear_position(nx, ny);
        let signed = wrapped_signed(position - phase);
        let width = self.band_half_width();
        let intensity = band_intensity(signed.abs(), width, self.feather());
        let band = self.sweep_color(signed, intensity, width.max(f32::EPSILON), palette);
        blend_srgba(palette.background, band, intensity)
    }

    fn render_opposing_sweeps(
        &self,
        nx: f32,
        ny: f32,
        phase: f32,
        palette: &CalibrationPalette,
    ) -> Srgba {
        let position = self.direction.linear_position(nx, ny);
        let width = self.band_half_width();
        let feather = self.feather();
        let first = wrapped_signed(position - phase);
        let second = wrapped_signed(position - (phase + 0.5).rem_euclid(1.0));
        let first_intensity = band_intensity(first.abs(), width, feather);
        let second_intensity = band_intensity(second.abs(), width, feather);

        let mut color = blend(
            palette.background,
            self.sweep_color(first, first_intensity, width.max(f32::EPSILON), palette),
            first_intensity,
        );
        color = blend(
            color,
            self.sweep_color(-second, second_intensity, width.max(f32::EPSILON), palette),
            second_intensity,
        );

        let overlap = (first_intensity * second_intensity).sqrt();
        blend_srgba(color, palette.accent, overlap)
    }

    fn render_crosshair(
        &self,
        nx: f32,
        ny: f32,
        phase: f32,
        palette: &CalibrationPalette,
    ) -> Srgba {
        let (cx, cy) = self.direction.crosshair_center(phase);
        let width = self.band_half_width();
        let feather = self.feather();
        let vertical = band_intensity((nx - cx).abs(), width, feather);
        let horizontal = band_intensity((ny - cy).abs(), width, feather);
        let intersection = (vertical * horizontal).sqrt();

        let color = blend_srgba(palette.background, palette.primary, vertical);
        let color = blend_srgba(color, palette.secondary, horizontal);
        blend_srgba(color, palette.accent, intersection)
    }

    fn render_quadrant_cycle(
        &self,
        nx: f32,
        ny: f32,
        phase: f32,
        palette: &CalibrationPalette,
    ) -> Srgba {
        let base_index = quadrant_index(nx, ny);
        let active = sequence_index(self.direction, phase);
        let subphase = (phase * 4.0).fract();
        let pulse = 0.72 + (0.28 * (subphase * TAU).sin().abs());
        let emphasis = if base_index == active { pulse } else { 0.34 };
        let mut color = blend_srgba(
            palette.background,
            palette.orientation[base_index],
            emphasis,
        );

        let divider = band_intensity((nx - 0.5).abs(), 0.01, 0.006).max(band_intensity(
            (ny - 0.5).abs(),
            0.01,
            0.006,
        ));
        color = blend_srgba(color, palette.accent, divider * 0.28);
        color
    }

    fn render_corner_cycle(
        &self,
        nx: f32,
        ny: f32,
        phase: f32,
        palette: &CalibrationPalette,
    ) -> Srgba {
        let extent = self.marker_extent().min(0.48);
        let active = sequence_index(self.direction, phase);
        let subphase = (phase * 4.0).fract();
        let pulse = 0.76 + (0.24 * (subphase * TAU).sin().abs());
        let mut color = palette.background;

        for index in 0..4 {
            let marker = corner_marker_intensity(index, nx, ny, extent);
            if marker <= 0.0 {
                continue;
            }

            let gain = if index == active { pulse } else { 0.30 };
            color = blend_srgba(
                color,
                palette.orientation[index],
                (marker * gain).clamp(0.0, 1.0),
            );
        }

        color
    }

    fn render_rings(&self, nx: f32, ny: f32, phase: f32, palette: &CalibrationPalette) -> Srgba {
        let dx = nx - 0.5;
        let dy = ny - 0.5;
        let radius = dx.hypot(dy) / 0.707_106_77;
        let width = (0.015 + (self.size.clamp(1.0, 100.0) / 100.0) * 0.06).clamp(0.01, 0.12);
        let feather = width * (self.softness.clamp(0.0, 100.0) / 100.0);
        let ring_density = 1.5 + (self.size.clamp(1.0, 100.0) / 100.0) * 4.5;
        let travel = match self.direction {
            CalibrationDirection::Inward => 1.0 - phase,
            _ => phase,
        };
        let repeated = (radius * ring_density - travel).rem_euclid(1.0);
        let distance = repeated.min(1.0 - repeated);
        let intensity = band_intensity(distance, width, feather);

        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::as_conversions,
            reason = "ring parity is only used for a local alternating palette choice"
        )]
        let ring_index = ((radius * ring_density - travel).floor() as i32).rem_euclid(2);

        let ring_color = if ring_index == 0 {
            palette.primary
        } else {
            palette.secondary
        };

        let mut color = blend_srgba(palette.background, ring_color, intensity);
        let center = band_intensity(radius, width * 0.6, feather * 0.6);
        color = blend_srgba(color, palette.accent, center * 0.6);
        color
    }

    fn grid_overlay(&self, nx: f32, ny: f32, width: f32, height: f32) -> f32 {
        if !self.show_grid {
            return 0.0;
        }

        let cols = self.grid_scale.clamp(2.0, 16.0).round();
        let rows = (cols * (height / width)).max(2.0).round();
        let dx = distance_to_grid(nx, cols);
        let dy = distance_to_grid(ny, rows);
        let grid = 1.0 - smoothstep(0.0, 0.08, dx.min(dy));
        let center = band_intensity((nx - 0.5).abs(), 0.02, 0.01).max(band_intensity(
            (ny - 0.5).abs(),
            0.02,
            0.01,
        ));
        grid.max(center * 0.9)
    }
}

impl Default for CalibrationRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectRenderer for CalibrationRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> anyhow::Result<()> {
        Ok(())
    }

    #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
    fn render_into(&mut self, input: &FrameInput<'_>, canvas: &mut Canvas) -> anyhow::Result<()> {
        prepare_target_canvas(canvas, input.canvas_width, input.canvas_height);
        let width = input.canvas_width.max(1) as f32;
        let height = input.canvas_height.max(1) as f32;
        let phase = self.phase(input.time_secs);

        let palette = self.palette();
        let row_stride = input.canvas_width as usize * BYTES_PER_PIXEL;
        let pixels = canvas.as_rgba_bytes_mut();

        for y in 0..input.canvas_height {
            let ny = (y as f32 + 0.5) / height;
            let row_offset = y as usize * row_stride;
            let row = &mut pixels[row_offset..row_offset + row_stride];
            for (x, pixel) in row.chunks_exact_mut(BYTES_PER_PIXEL).enumerate() {
                let nx = (x as f32 + 0.5) / width;
                let mut color = match self.pattern {
                    CalibrationPattern::Sweep => self.render_sweep(nx, ny, phase, &palette),
                    CalibrationPattern::OpposingSweeps => {
                        self.render_opposing_sweeps(nx, ny, phase, &palette)
                    }
                    CalibrationPattern::Crosshair => self.render_crosshair(nx, ny, phase, &palette),
                    CalibrationPattern::QuadrantCycle => {
                        self.render_quadrant_cycle(nx, ny, phase, &palette)
                    }
                    CalibrationPattern::CornerCycle => {
                        self.render_corner_cycle(nx, ny, phase, &palette)
                    }
                    CalibrationPattern::Rings => self.render_rings(nx, ny, phase, &palette),
                };

                if self.show_grid {
                    let grid = self.grid_overlay(nx, ny, width, height);
                    if grid > 0.0 {
                        color = blend_srgba(color, palette.accent, grid * 0.35);
                    }
                }

                pixel.copy_from_slice(&color);
            }
        }

        Ok(())
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        match name {
            "pattern" => {
                if let ControlValue::Enum(choice) | ControlValue::Text(choice) = value {
                    self.pattern = CalibrationPattern::from_str(choice);
                }
            }
            "direction" => {
                if let ControlValue::Enum(choice) | ControlValue::Text(choice) = value {
                    self.direction = CalibrationDirection::from_str(choice);
                }
            }
            "primary_color" => {
                if let ControlValue::Color(color) = value {
                    self.primary_color = *color;
                }
            }
            "secondary_color" => {
                if let ControlValue::Color(color) = value {
                    self.secondary_color = *color;
                }
            }
            "accent_color" => {
                if let ControlValue::Color(color) = value {
                    self.accent_color = *color;
                }
            }
            "background_color" => {
                if let ControlValue::Color(color) = value {
                    self.background_color = *color;
                }
            }
            "speed" => {
                if let Some(speed) = value.as_f32() {
                    self.speed = speed.clamp(0.0, 100.0);
                }
            }
            "size" => {
                if let Some(size) = value.as_f32() {
                    self.size = size.clamp(1.0, 100.0);
                }
            }
            "softness" => {
                if let Some(softness) = value.as_f32() {
                    self.softness = softness.clamp(0.0, 100.0);
                }
            }
            "show_grid" => {
                if let ControlValue::Boolean(show_grid) = value {
                    self.show_grid = *show_grid;
                }
            }
            "grid_scale" => {
                if let Some(grid_scale) = value.as_f32() {
                    self.grid_scale = grid_scale.clamp(2.0, 16.0);
                }
            }
            "brightness" => {
                if let Some(brightness) = value.as_f32() {
                    self.brightness = brightness.clamp(0.0, 1.0);
                }
            }
            _ => {}
        }
    }

    fn destroy(&mut self) {}
}

fn blend(base: Srgba, overlay: Srgba, amount: f32) -> Srgba {
    blend_srgba(base, overlay, amount)
}

fn blend_srgba(base: Srgba, overlay: Srgba, amount: f32) -> Srgba {
    let amount = amount.clamp(0.0, 1.0);
    [
        mix_channel(base[0], overlay[0], amount),
        mix_channel(base[1], overlay[1], amount),
        mix_channel(base[2], overlay[2], amount),
        mix_channel(base[3], overlay[3], amount),
    ]
}

fn mix_channel(base: u8, overlay: u8, amount: f32) -> u8 {
    let base = f32::from(base);
    let overlay = f32::from(overlay);
    (base + (overlay - base) * amount).round().clamp(0.0, 255.0) as u8
}

fn band_intensity(distance: f32, half_width: f32, feather: f32) -> f32 {
    if distance >= half_width {
        return 0.0;
    }

    if feather <= f32::EPSILON {
        return 1.0;
    }

    let solid = (half_width - feather).max(0.0);
    if distance <= solid {
        1.0
    } else {
        1.0 - smoothstep(solid, half_width, distance)
    }
}

fn distance_to_grid(value: f32, divisions: f32) -> f32 {
    let scaled = value * divisions;
    let fract = scaled.fract().abs();
    fract.min(1.0 - fract)
}

fn quadrant_index(nx: f32, ny: f32) -> usize {
    match (nx >= 0.5, ny >= 0.5) {
        (false, false) => 0,
        (true, false) => 1,
        (true, true) => 2,
        (false, true) => 3,
    }
}

fn sequence_index(direction: CalibrationDirection, phase: f32) -> usize {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::as_conversions,
        reason = "phase is normalized to 0..1 before the quadrant sequence lookup"
    )]
    let step = ((phase * 4.0).floor() as usize) % 4;

    match direction {
        CalibrationDirection::CounterClockwise => [0, 3, 2, 1][step],
        _ => [0, 1, 2, 3][step],
    }
}

fn corner_marker_intensity(index: usize, nx: f32, ny: f32, extent: f32) -> f32 {
    let arm = (extent * 0.22).max(0.035);
    match index {
        0 => {
            if (nx <= extent && ny <= arm) || (nx <= arm && ny <= extent) {
                1.0
            } else {
                0.0
            }
        }
        1 => {
            if ((1.0 - nx) <= extent && ny <= arm) || ((1.0 - nx) <= arm && ny <= extent) {
                1.0
            } else {
                0.0
            }
        }
        2 => {
            if ((1.0 - nx) <= extent && (1.0 - ny) <= arm)
                || ((1.0 - nx) <= arm && (1.0 - ny) <= extent)
            {
                1.0
            } else {
                0.0
            }
        }
        3 => {
            if (nx <= extent && (1.0 - ny) <= arm) || (nx <= arm && (1.0 - ny) <= extent) {
                1.0
            } else {
                0.0
            }
        }
        _ => 0.0,
    }
}

fn normalize_choice(value: &str) -> String {
    let mut normalized = String::new();
    let mut last_was_separator = false;

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator {
            normalized.push('_');
            last_was_separator = true;
        }
    }

    normalized.trim_matches('_').to_owned()
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    if (edge1 - edge0).abs() <= f32::EPSILON {
        return if value >= edge1 { 1.0 } else { 0.0 };
    }

    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn wrapped_signed(value: f32) -> f32 {
    (value + 0.5).rem_euclid(1.0) - 0.5
}

fn controls() -> Vec<ControlDefinition> {
    vec![
        dropdown_control(
            "pattern",
            "Pattern",
            "Sweep",
            &[
                "Sweep",
                "Opposing Sweeps",
                "Crosshair",
                "Quadrant Cycle",
                "Corner Cycle",
                "Rings",
            ],
            "Pattern",
            "Choose the calibration scene for placement, rotation, or coverage checks.",
        ),
        dropdown_control(
            "direction",
            "Direction",
            "Left to Right",
            &[
                "Left to Right",
                "Right to Left",
                "Top to Bottom",
                "Bottom to Top",
                "Top Left to Bottom Right",
                "Bottom Right to Top Left",
                "Top Right to Bottom Left",
                "Bottom Left to Top Right",
                "Clockwise",
                "Counter Clockwise",
                "Outward",
                "Inward",
            ],
            "Motion",
            "Flow direction for sweeps, crosshairs, corner order, or ring travel.",
        ),
        slider_control(
            "speed",
            "Sweep Speed",
            18.0,
            0.0,
            100.0,
            1.0,
            "Motion",
            "Keep this low for deliberate setup passes; raise it for faster scanning.",
        ),
        slider_control(
            "size",
            "Marker Size",
            22.0,
            1.0,
            100.0,
            1.0,
            "Motion",
            "Width of sweep bands, crosshair bars, corner markers, or ring thickness.",
        ),
        slider_control(
            "softness",
            "Edge Softness",
            18.0,
            0.0,
            100.0,
            1.0,
            "Motion",
            "Feather edges for softer bands, or drop it for crisp diagnostic boundaries.",
        ),
        color_control(
            "primary_color",
            "Lead Color",
            LEAD_COLOR,
            "Colors",
            "Leading edge or primary scan color.",
        ),
        color_control(
            "secondary_color",
            "Trail Color",
            TRAIL_COLOR,
            "Colors",
            "Trailing edge or secondary scan color.",
        ),
        color_control(
            "accent_color",
            "Accent Color",
            ACCENT_COLOR,
            "Colors",
            "Used for intersections, grid overlays, and hotspot emphasis.",
        ),
        color_control(
            "background_color",
            "Background Color",
            BACKGROUND_COLOR,
            "Colors",
            "Dark backdrop that the diagnostic patterns ride on top of.",
        ),
        toggle_control(
            "show_grid",
            "Show Grid Overlay",
            false,
            "Layout",
            "Overlay a coarse grid and center lines to estimate footprint and spacing.",
        ),
        slider_control(
            "grid_scale",
            "Grid Scale",
            8.0,
            2.0,
            16.0,
            1.0,
            "Layout",
            "Number of major grid divisions across the canvas width.",
        ),
        slider_control(
            "brightness",
            "Brightness",
            1.0,
            0.0,
            1.0,
            0.01,
            "Output",
            "Master output brightness.",
        ),
    ]
}

#[expect(
    clippy::too_many_lines,
    reason = "calibration presets intentionally cover several setup workflows"
)]
fn presets() -> Vec<PresetTemplate> {
    vec![
        preset_with_desc(
            "Horizontal Sweep",
            "Slow left-to-right pass for rough device placement and strip direction checks",
            &[
                ("pattern", ControlValue::Enum("Sweep".to_owned())),
                ("direction", ControlValue::Enum("Left to Right".to_owned())),
                ("speed", ControlValue::Float(18.0)),
                ("size", ControlValue::Float(20.0)),
                ("softness", ControlValue::Float(12.0)),
                ("show_grid", ControlValue::Boolean(false)),
            ],
        ),
        preset_with_desc(
            "Vertical Sweep",
            "Top-to-bottom pass for stacked layouts, towers, and vertical strips",
            &[
                ("pattern", ControlValue::Enum("Sweep".to_owned())),
                ("direction", ControlValue::Enum("Top to Bottom".to_owned())),
                ("speed", ControlValue::Float(18.0)),
                ("size", ControlValue::Float(20.0)),
                ("softness", ControlValue::Float(12.0)),
                ("show_grid", ControlValue::Boolean(false)),
            ],
        ),
        preset_with_desc(
            "Opposing Edge Scan",
            "Two mirrored sweeps that make center alignment and mirrored mistakes obvious",
            &[
                ("pattern", ControlValue::Enum("Opposing Sweeps".to_owned())),
                ("direction", ControlValue::Enum("Left to Right".to_owned())),
                ("speed", ControlValue::Float(16.0)),
                ("size", ControlValue::Float(16.0)),
                ("softness", ControlValue::Float(10.0)),
                ("show_grid", ControlValue::Boolean(true)),
                ("grid_scale", ControlValue::Float(8.0)),
            ],
        ),
        preset_with_desc(
            "Diagonal Crosshair",
            "Moving vertical and horizontal bars whose intersection walks the layout diagonally",
            &[
                ("pattern", ControlValue::Enum("Crosshair".to_owned())),
                (
                    "direction",
                    ControlValue::Enum("Top Left to Bottom Right".to_owned()),
                ),
                ("speed", ControlValue::Float(22.0)),
                ("size", ControlValue::Float(14.0)),
                ("softness", ControlValue::Float(16.0)),
                ("show_grid", ControlValue::Boolean(true)),
                ("grid_scale", ControlValue::Float(10.0)),
            ],
        ),
        preset_with_desc(
            "Quadrant Clock",
            "Clockwise quadrant cycling to verify global orientation at a glance",
            &[
                ("pattern", ControlValue::Enum("Quadrant Cycle".to_owned())),
                ("direction", ControlValue::Enum("Clockwise".to_owned())),
                ("speed", ControlValue::Float(20.0)),
                ("size", ControlValue::Float(24.0)),
                ("softness", ControlValue::Float(8.0)),
            ],
        ),
        preset_with_desc(
            "Corner Compass",
            "Corner beacons cycle around the canvas to expose rotation and mirrored placements",
            &[
                ("pattern", ControlValue::Enum("Corner Cycle".to_owned())),
                ("direction", ControlValue::Enum("Clockwise".to_owned())),
                ("speed", ControlValue::Float(20.0)),
                ("size", ControlValue::Float(34.0)),
                ("softness", ControlValue::Float(0.0)),
            ],
        ),
        preset_with_desc(
            "Expanding Rings",
            "Concentric rings from the center for scale, centering, and radial coverage checks",
            &[
                ("pattern", ControlValue::Enum("Rings".to_owned())),
                ("direction", ControlValue::Enum("Outward".to_owned())),
                ("speed", ControlValue::Float(16.0)),
                ("size", ControlValue::Float(44.0)),
                ("softness", ControlValue::Float(20.0)),
                ("show_grid", ControlValue::Boolean(true)),
                ("grid_scale", ControlValue::Float(8.0)),
            ],
        ),
        preset_with_desc(
            "Inbound Rings",
            "Reverse ring motion to confirm center-in vs center-out assumptions",
            &[
                ("pattern", ControlValue::Enum("Rings".to_owned())),
                ("direction", ControlValue::Enum("Inward".to_owned())),
                ("speed", ControlValue::Float(16.0)),
                ("size", ControlValue::Float(44.0)),
                ("softness", ControlValue::Float(20.0)),
                ("show_grid", ControlValue::Boolean(true)),
                ("grid_scale", ControlValue::Float(8.0)),
            ],
        ),
    ]
}

pub(super) fn metadata() -> EffectMetadata {
    EffectMetadata {
        id: builtin_effect_id("calibration"),
        name: "Calibration".into(),
        author: "Hypercolor".into(),
        version: "0.1.0".into(),
        description:
            "High-contrast sweeps, crosshairs, quadrants, corners, and rings for layout setup"
                .into(),
        category: EffectCategory::Utility,
        tags: vec![
            "calibration".into(),
            "diagnostic".into(),
            "layout".into(),
            "orientation".into(),
            "rotation".into(),
            "utility".into(),
        ],
        controls: controls(),
        presets: presets(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from("builtin/calibration"),
        },
        license: Some("Apache-2.0".into()),
    }
}
