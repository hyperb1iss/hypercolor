use std::cmp::{max, min};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::anyhow;

use hypercolor_core::blend_math::blend_rgba_pixels_in_place;
use hypercolor_core::overlay::{
    ClockRenderer, ImageRenderer, OverlayBuffer, OverlayError, OverlayInput, OverlayRenderer,
    OverlaySize, SensorRenderer, TextRenderer,
};
use hypercolor_types::overlay::{
    Anchor, ClockConfig, ClockStyle, DisplayOverlayConfig, OverlayBlendMode, OverlayPosition,
    OverlaySlot, OverlaySource,
};
use hypercolor_types::sensor::SystemSnapshot;

use crate::display_overlays::{DisplayOverlayRuntime, OverlaySlotRuntime, OverlaySlotStatus};

pub trait OverlayRendererFactory: Send + Sync {
    fn build(
        &self,
        slot: &OverlaySlot,
        target_size: OverlaySize,
    ) -> Result<OverlayRendererBinding, OverlayError>;
}

pub struct OverlayRendererBinding {
    pub renderer: Box<dyn OverlayRenderer>,
    pub render_interval: Duration,
}

pub struct DefaultOverlayRendererFactory;

impl DefaultOverlayRendererFactory {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl OverlayRendererFactory for DefaultOverlayRendererFactory {
    fn build(
        &self,
        slot: &OverlaySlot,
        _target_size: OverlaySize,
    ) -> Result<OverlayRendererBinding, OverlayError> {
        match &slot.source {
            OverlaySource::Clock(config) => Ok(OverlayRendererBinding {
                renderer: Box::new(
                    ClockRenderer::new(config.clone()).map_err(OverlayError::Asset)?,
                ),
                render_interval: clock_render_interval(config),
            }),
            OverlaySource::Image(config) => Ok(OverlayRendererBinding {
                renderer: Box::new(
                    ImageRenderer::new(config.clone()).map_err(OverlayError::Asset)?,
                ),
                render_interval: Duration::MAX,
            }),
            OverlaySource::Text(config) => Ok(OverlayRendererBinding {
                renderer: Box::new(TextRenderer::new(config.clone()).map_err(OverlayError::Asset)?),
                render_interval: text_render_interval(config),
            }),
            OverlaySource::Sensor(config) => Ok(OverlayRendererBinding {
                renderer: Box::new(
                    SensorRenderer::new(config.clone()).map_err(OverlayError::Asset)?,
                ),
                render_interval: sensor_render_interval(),
            }),
            source => Err(OverlayError::Asset(anyhow!(
                "overlay renderer is not implemented yet for source {source:?}"
            ))),
        }
    }
}

fn text_render_interval(config: &hypercolor_types::overlay::TextOverlayConfig) -> Duration {
    if config.scroll {
        return Duration::from_millis(33);
    }
    if config.text.contains("{sensor:") {
        return Duration::from_secs(2);
    }

    Duration::MAX
}

fn clock_render_interval(config: &ClockConfig) -> Duration {
    if matches!(config.style, ClockStyle::Analog) && config.show_seconds {
        return Duration::from_millis(500);
    }

    Duration::from_secs(1)
}

fn sensor_render_interval() -> Duration {
    Duration::from_secs(2)
}

#[doc(hidden)]
pub struct OverlayComposer {
    instances: Vec<OverlayInstance>,
    staging: PremulStaging,
    display_width: u32,
    display_height: u32,
    circular: bool,
    created_at: Instant,
    factory: Arc<dyn OverlayRendererFactory>,
}

impl OverlayComposer {
    pub fn new(
        display_width: u32,
        display_height: u32,
        circular: bool,
        factory: Arc<dyn OverlayRendererFactory>,
    ) -> Self {
        Self {
            instances: Vec::new(),
            staging: PremulStaging::new(display_width, display_height),
            display_width,
            display_height,
            circular,
            created_at: Instant::now(),
            factory,
        }
    }

    #[doc(hidden)]
    pub fn reconcile(&mut self, config: &DisplayOverlayConfig) {
        self.destroy_instances();
        self.instances = config
            .overlays
            .iter()
            .cloned()
            .map(|slot| self.build_instance(slot.normalized()))
            .collect();
    }

    pub fn has_active_slots(&self) -> bool {
        self.instances.iter().any(OverlayInstance::is_active)
    }

    #[must_use]
    pub fn runtime_snapshot(&self) -> DisplayOverlayRuntime {
        DisplayOverlayRuntime {
            slots: self
                .instances
                .iter()
                .map(|instance| (instance.slot.id, instance.runtime()))
                .collect(),
        }
    }

    pub fn next_refresh_at(&self, now: Instant) -> Option<Instant> {
        self.instances
            .iter()
            .filter_map(|instance| instance.next_refresh_at(now))
            .min()
    }

    pub fn compose_rgb_frame<'a>(
        &'a mut self,
        base_rgb: &[u8],
        sensors: &SystemSnapshot,
        frame_number: u64,
        now_system: SystemTime,
        now_instant: Instant,
    ) -> Option<&'a PremulStaging> {
        self.compose_rgb_frame_with_runtime_change(
            base_rgb,
            sensors,
            frame_number,
            now_system,
            now_instant,
        )
        .0
    }

    pub fn compose_rgb_frame_with_runtime_change<'a>(
        &'a mut self,
        base_rgb: &[u8],
        sensors: &SystemSnapshot,
        frame_number: u64,
        now_system: SystemTime,
        now_instant: Instant,
    ) -> (Option<&'a PremulStaging>, bool) {
        if self.instances.is_empty() {
            return (None, false);
        }

        let mut has_active_slot = false;
        let mut runtime_changed = false;
        let mut input = None::<OverlayInput<'_>>;

        for instance in &mut self.instances {
            if !instance.slot.enabled || instance.disabled {
                continue;
            }
            if !has_active_slot {
                self.staging
                    .write_from_rgb(base_rgb, self.display_width, self.display_height);
                has_active_slot = true;
            }
            let input = input.get_or_insert_with(|| OverlayInput {
                now: now_system,
                display_width: self.display_width,
                display_height: self.display_height,
                circular: self.circular,
                sensors,
                elapsed_secs: now_instant.duration_since(self.created_at).as_secs_f32(),
                frame_number,
            });

            runtime_changed |= instance.maybe_render(input, now_instant);
            if !instance.has_valid_render {
                continue;
            }

            blend_slot_into_staging(
                &mut self.staging,
                &instance.cached_buffer,
                &instance.slot.position,
                instance.slot.blend_mode,
                instance.slot.opacity,
            );
        }

        if !has_active_slot {
            return (None, runtime_changed);
        }

        (Some(&self.staging), runtime_changed)
    }

    fn build_instance(&self, slot: OverlaySlot) -> OverlayInstance {
        let target_size =
            overlay_target_size(&slot.position, self.display_width, self.display_height);
        match self.factory.build(&slot, target_size) {
            Ok(binding) => OverlayInstance::new(slot, binding, target_size),
            Err(error) => OverlayInstance::disabled(slot, target_size, error),
        }
    }

    fn destroy_instances(&mut self) {
        for instance in &mut self.instances {
            instance.destroy();
        }
        self.instances.clear();
    }
}

impl Drop for OverlayComposer {
    fn drop(&mut self) {
        self.destroy_instances();
    }
}

struct OverlayInstance {
    slot: OverlaySlot,
    renderer: Option<Box<dyn OverlayRenderer>>,
    cached_buffer: OverlayBuffer,
    has_valid_render: bool,
    last_rendered_at: Option<Instant>,
    last_rendered_at_wall: Option<SystemTime>,
    render_interval: Duration,
    consecutive_failures: u32,
    backoff_until: Option<Instant>,
    last_error: Option<OverlayError>,
    disabled: bool,
}

impl OverlayInstance {
    fn new(slot: OverlaySlot, binding: OverlayRendererBinding, target_size: OverlaySize) -> Self {
        let mut renderer = binding.renderer;
        let init_result = renderer.init(target_size).map_err(OverlayError::Asset);
        let mut instance = Self {
            slot,
            renderer: Some(renderer),
            cached_buffer: OverlayBuffer::new(target_size),
            has_valid_render: false,
            last_rendered_at: None,
            last_rendered_at_wall: None,
            render_interval: binding.render_interval.max(Duration::from_millis(16)),
            consecutive_failures: 0,
            backoff_until: None,
            last_error: None,
            disabled: false,
        };
        if let Err(error) = init_result {
            instance.handle_error(error, Instant::now());
        }
        instance
    }

    fn disabled(slot: OverlaySlot, target_size: OverlaySize, error: OverlayError) -> Self {
        Self {
            slot,
            renderer: None,
            cached_buffer: OverlayBuffer::new(target_size),
            has_valid_render: false,
            last_rendered_at: None,
            last_rendered_at_wall: None,
            render_interval: Duration::from_secs(1),
            consecutive_failures: 0,
            backoff_until: None,
            last_error: Some(error),
            disabled: true,
        }
    }

    fn is_active(&self) -> bool {
        self.slot.enabled && !self.disabled
    }

    fn runtime(&self) -> OverlaySlotRuntime {
        let status = if !self.slot.enabled {
            OverlaySlotStatus::Disabled
        } else if self.disabled {
            OverlaySlotStatus::Failed
        } else {
            OverlaySlotStatus::Active
        };

        OverlaySlotRuntime {
            last_rendered_at: self.last_rendered_at_wall,
            consecutive_failures: self.consecutive_failures,
            last_error: self.last_error.as_ref().map(ToString::to_string),
            status,
        }
    }

    fn next_refresh_at(&self, now: Instant) -> Option<Instant> {
        if !self.is_active() || self.renderer.is_none() {
            return None;
        }
        if let Some(deadline) = self.backoff_until {
            return Some(deadline);
        }
        if !self.has_valid_render {
            return Some(now);
        }
        if let Some(refresh_after) = self
            .renderer
            .as_ref()
            .and_then(|renderer| renderer.next_refresh_after())
        {
            return self
                .last_rendered_at
                .and_then(|last| last.checked_add(refresh_after));
        }
        self.last_rendered_at
            .and_then(|last| last.checked_add(self.render_interval))
    }

    fn maybe_render(&mut self, input: &OverlayInput<'_>, now: Instant) -> bool {
        let Some(renderer) = self.renderer.as_mut() else {
            return false;
        };
        if self.backoff_until.is_some_and(|deadline| deadline > now) {
            return false;
        }

        let cadence_due = self
            .last_rendered_at
            .is_none_or(|last| now.duration_since(last) >= self.render_interval);
        let content_dirty = renderer.content_changed(input);
        if !cadence_due && !content_dirty && self.has_valid_render {
            return false;
        }

        self.cached_buffer.clear();
        match renderer.render_into(input, &mut self.cached_buffer) {
            Ok(()) => {
                self.has_valid_render = true;
                self.last_rendered_at = Some(now);
                self.last_rendered_at_wall = Some(copied_system_time(&input.now));
                self.consecutive_failures = 0;
                self.backoff_until = None;
                self.last_error = None;
                true
            }
            Err(error) => {
                self.handle_error(error, now);
                true
            }
        }
    }

    fn handle_error(&mut self, error: OverlayError, now: Instant) {
        match error {
            OverlayError::Asset(error) => {
                self.has_valid_render = false;
                self.disabled = true;
                self.backoff_until = None;
                self.last_error = Some(OverlayError::Asset(error));
                self.destroy();
            }
            OverlayError::Fatal(error) => {
                self.has_valid_render = false;
                self.disabled = true;
                self.backoff_until = None;
                self.last_error = Some(OverlayError::Fatal(error));
                self.destroy();
            }
            OverlayError::Transient(error) => {
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                self.backoff_until = now.checked_add(backoff_duration(self.consecutive_failures));
                if self.consecutive_failures >= 5 {
                    self.backoff_until = None;
                    self.disabled = true;
                    self.last_error = Some(OverlayError::Asset(anyhow!(
                        "disabled after {} consecutive transient failures: {error}",
                        self.consecutive_failures
                    )));
                    self.destroy();
                    return;
                }
                self.last_error = Some(OverlayError::Transient(error));
            }
        }
    }

    fn destroy(&mut self) {
        if let Some(mut renderer) = self.renderer.take() {
            renderer.destroy();
        }
    }
}

pub struct PremulStaging {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    fully_opaque: bool,
}

impl PremulStaging {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            pixels: vec![0; pixel_len(width, height, 4)],
            width,
            height,
            fully_opaque: true,
        }
    }

    pub fn write_from_rgb(&mut self, source: &[u8], width: u32, height: u32) {
        self.resize(width, height);
        self.fully_opaque = true;
        for (rgba, rgb) in self.pixels.chunks_exact_mut(4).zip(source.chunks_exact(3)) {
            rgba[0] = rgb[0];
            rgba[1] = rgb[1];
            rgba[2] = rgb[2];
            rgba[3] = u8::MAX;
        }
    }

    pub fn write_from_straight_rgba(&mut self, source: &[u8], width: u32, height: u32) {
        self.resize(width, height);
        let mut fully_opaque = true;
        for (premul, straight) in self.pixels.chunks_exact_mut(4).zip(source.chunks_exact(4)) {
            let alpha = u16::from(straight[3]);
            premul[3] = straight[3];
            if alpha == 0 {
                fully_opaque = false;
                premul[0] = 0;
                premul[1] = 0;
                premul[2] = 0;
                continue;
            }
            if alpha == u16::from(u8::MAX) {
                premul[0] = straight[0];
                premul[1] = straight[1];
                premul[2] = straight[2];
                continue;
            }
            fully_opaque = false;
            premul[0] = premultiply_channel(straight[0], straight[3]);
            premul[1] = premultiply_channel(straight[1], straight[3]);
            premul[2] = premultiply_channel(straight[2], straight[3]);
        }
        self.fully_opaque = fully_opaque;
    }

    pub fn write_into_rgb(&self, target: &mut Vec<u8>) {
        let target_len = pixel_len(self.width, self.height, 3);
        if target.len() != target_len {
            target.resize(target_len, 0);
        }
        if self.fully_opaque {
            for (rgb, premul) in target.chunks_exact_mut(3).zip(self.pixels.chunks_exact(4)) {
                rgb.copy_from_slice(&premul[..3]);
            }
            return;
        }
        for (rgb, premul) in target.chunks_exact_mut(3).zip(self.pixels.chunks_exact(4)) {
            let alpha = premul[3];
            if alpha == 0 {
                rgb[0] = 0;
                rgb[1] = 0;
                rgb[2] = 0;
                continue;
            }
            if alpha == u8::MAX {
                rgb[0] = premul[0];
                rgb[1] = premul[1];
                rgb[2] = premul[2];
                continue;
            }
            rgb[0] = unpremultiply_channel(premul[0], alpha);
            rgb[1] = unpremultiply_channel(premul[1], alpha);
            rgb[2] = unpremultiply_channel(premul[2], alpha);
        }
    }

    pub fn write_into_straight_rgba(&self, target: &mut Vec<u8>) {
        let target_len = pixel_len(self.width, self.height, 4);
        if target.len() != target_len {
            target.resize(target_len, 0);
        }
        if self.fully_opaque {
            for (straight, premul) in target.chunks_exact_mut(4).zip(self.pixels.chunks_exact(4)) {
                straight[..3].copy_from_slice(&premul[..3]);
                straight[3] = u8::MAX;
            }
            return;
        }
        for (straight, premul) in target.chunks_exact_mut(4).zip(self.pixels.chunks_exact(4)) {
            let alpha = premul[3];
            straight[3] = alpha;
            if alpha == 0 {
                straight[0] = 0;
                straight[1] = 0;
                straight[2] = 0;
                continue;
            }
            if alpha == u8::MAX {
                straight[0] = premul[0];
                straight[1] = premul[1];
                straight[2] = premul[2];
                continue;
            }
            straight[0] = unpremultiply_channel(premul[0], alpha);
            straight[1] = unpremultiply_channel(premul[1], alpha);
            straight[2] = unpremultiply_channel(premul[2], alpha);
        }
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.pixels.resize(pixel_len(width, height, 4), 0);
    }
}

fn overlay_target_size(
    position: &OverlayPosition,
    display_width: u32,
    display_height: u32,
) -> OverlaySize {
    match position {
        OverlayPosition::FullScreen => {
            OverlaySize::new(display_width.max(1), display_height.max(1))
        }
        OverlayPosition::Anchored { width, height, .. } => {
            OverlaySize::new((*width).max(1), (*height).max(1))
        }
    }
}

fn blend_slot_into_staging(
    staging: &mut PremulStaging,
    buffer: &OverlayBuffer,
    position: &OverlayPosition,
    blend_mode: OverlayBlendMode,
    opacity: f32,
) {
    let rect = resolved_rect(position, staging.width, staging.height);
    if rect.width == 0 || rect.height == 0 {
        return;
    }

    let clip_left = max(rect.x, 0);
    let clip_top = max(rect.y, 0);
    let clip_right = min(
        rect.x
            .saturating_add(i32::try_from(rect.width).unwrap_or(i32::MAX)),
        i32::try_from(staging.width).unwrap_or_default(),
    );
    let clip_bottom = min(
        rect.y
            .saturating_add(i32::try_from(rect.height).unwrap_or(i32::MAX)),
        i32::try_from(staging.height).unwrap_or_default(),
    );
    if clip_left >= clip_right || clip_top >= clip_bottom {
        return;
    }

    let row_width = usize::try_from(clip_right - clip_left).unwrap_or_default();
    let row_bytes = row_width.saturating_mul(4);
    let src_x = u32::try_from(clip_left - rect.x).unwrap_or_default();

    for y in clip_top..clip_bottom {
        let dst_offset = pixel_offset(
            staging.width,
            u32::try_from(clip_left).unwrap_or_default(),
            u32::try_from(y).unwrap_or_default(),
        );
        let src_offset = pixel_offset(
            buffer.width,
            src_x,
            u32::try_from(y - rect.y).unwrap_or_default(),
        );
        let dst_row = &mut staging.pixels[dst_offset..dst_offset + row_bytes];
        let src_row = &buffer.pixels[src_offset..src_offset + row_bytes];
        blend_rgba_pixels_in_place(dst_row, src_row, blend_mode, opacity);
    }
}

fn resolved_rect(
    position: &OverlayPosition,
    display_width: u32,
    display_height: u32,
) -> ResolvedOverlayRect {
    match position {
        OverlayPosition::FullScreen => ResolvedOverlayRect {
            x: 0,
            y: 0,
            width: display_width,
            height: display_height,
        },
        OverlayPosition::Anchored {
            anchor,
            offset_x,
            offset_y,
            width,
            height,
        } => {
            let display_width_i32 = i32::try_from(display_width).unwrap_or(i32::MAX);
            let display_height_i32 = i32::try_from(display_height).unwrap_or(i32::MAX);
            let width_i32 = i32::try_from(*width).unwrap_or(i32::MAX);
            let height_i32 = i32::try_from(*height).unwrap_or(i32::MAX);
            let (base_x, base_y) = anchor_origin(
                *anchor,
                display_width_i32,
                display_height_i32,
                width_i32,
                height_i32,
            );
            ResolvedOverlayRect {
                x: base_x.saturating_add(*offset_x),
                y: base_y.saturating_add(*offset_y),
                width: *width,
                height: *height,
            }
        }
    }
}

fn anchor_origin(
    anchor: Anchor,
    display_width: i32,
    display_height: i32,
    width: i32,
    height: i32,
) -> (i32, i32) {
    match anchor {
        Anchor::TopLeft => (0, 0),
        Anchor::TopCenter => ((display_width - width) / 2, 0),
        Anchor::TopRight => (display_width - width, 0),
        Anchor::CenterLeft => (0, (display_height - height) / 2),
        Anchor::Center => ((display_width - width) / 2, (display_height - height) / 2),
        Anchor::CenterRight => (display_width - width, (display_height - height) / 2),
        Anchor::BottomLeft => (0, display_height - height),
        Anchor::BottomCenter => ((display_width - width) / 2, display_height - height),
        Anchor::BottomRight => (display_width - width, display_height - height),
    }
}

fn pixel_offset(width: u32, x: u32, y: u32) -> usize {
    let width = usize::try_from(width).unwrap_or_default();
    let x = usize::try_from(x).unwrap_or_default();
    let y = usize::try_from(y).unwrap_or_default();
    y.saturating_mul(width).saturating_add(x).saturating_mul(4)
}

fn pixel_len(width: u32, height: u32, channels: usize) -> usize {
    usize::try_from(width)
        .unwrap_or_default()
        .saturating_mul(usize::try_from(height).unwrap_or_default())
        .saturating_mul(channels)
}

fn premultiply_channel(channel: u8, alpha: u8) -> u8 {
    let scaled = u16::from(channel)
        .saturating_mul(u16::from(alpha))
        .saturating_add(127)
        / u16::from(u8::MAX);
    u8::try_from(scaled).unwrap_or(u8::MAX)
}

fn unpremultiply_channel(channel: u8, alpha: u8) -> u8 {
    if alpha == 0 {
        return 0;
    }
    if alpha == u8::MAX {
        return channel;
    }
    let scaled = u32::from(channel)
        .saturating_mul(u32::from(u8::MAX))
        .saturating_add(u32::from(alpha) / 2)
        / u32::from(alpha);
    u8::try_from(scaled.min(u32::from(u8::MAX))).unwrap_or(u8::MAX)
}

fn copied_system_time(time: &SystemTime) -> SystemTime {
    SystemTime::UNIX_EPOCH
        .checked_add(
            time.duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default(),
        )
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn backoff_duration(consecutive_failures: u32) -> Duration {
    let shift = consecutive_failures.saturating_sub(1).min(5);
    let millis = 500_u64.saturating_mul(1_u64 << shift);
    Duration::from_millis(millis.min(30_000))
}

struct ResolvedOverlayRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use hypercolor_types::overlay::{
        DisplayOverlayConfig, OverlayBlendMode, OverlayPosition, OverlaySlot, OverlaySlotId,
        OverlaySource, TextAlign, TextOverlayConfig,
    };
    use uuid::Uuid;

    struct SolidRenderer {
        color: [u8; 4],
    }

    impl OverlayRenderer for SolidRenderer {
        fn init(&mut self, _target_size: OverlaySize) -> Result<()> {
            Ok(())
        }

        fn resize(&mut self, _target_size: OverlaySize) -> Result<()> {
            Ok(())
        }

        fn render_into(
            &mut self,
            _input: &OverlayInput<'_>,
            target: &mut OverlayBuffer,
        ) -> std::result::Result<(), OverlayError> {
            for pixel in target.pixels.chunks_exact_mut(4) {
                pixel.copy_from_slice(&self.color);
            }
            Ok(())
        }
    }

    struct SolidFactory {
        color: [u8; 4],
        interval: Duration,
    }

    impl OverlayRendererFactory for SolidFactory {
        fn build(
            &self,
            _slot: &OverlaySlot,
            _target_size: OverlaySize,
        ) -> std::result::Result<OverlayRendererBinding, OverlayError> {
            Ok(OverlayRendererBinding {
                renderer: Box::new(SolidRenderer { color: self.color }),
                render_interval: self.interval,
            })
        }
    }

    fn sample_slot(position: OverlayPosition) -> OverlaySlot {
        OverlaySlot {
            id: OverlaySlotId::from(Uuid::now_v7()),
            name: "Test".to_owned(),
            source: OverlaySource::Text(TextOverlayConfig {
                text: "Test".to_owned(),
                font_family: None,
                font_size: 12.0,
                color: "#ffffff".to_owned(),
                align: TextAlign::Center,
                scroll: false,
                scroll_speed: 30.0,
            }),
            position,
            blend_mode: OverlayBlendMode::Normal,
            opacity: 0.5,
            enabled: true,
        }
    }

    #[test]
    fn premul_staging_round_trips_straight_rgba() {
        let mut staging = PremulStaging::new(2, 1);
        let source = vec![255, 128, 64, 255, 120, 60, 30, 128];
        let mut restored = Vec::new();

        staging.write_from_straight_rgba(&source, 2, 1);
        staging.write_into_straight_rgba(&mut restored);

        assert_eq!(&restored[..4], &source[..4]);
        assert!((i16::from(restored[4]) - i16::from(source[4])).abs() <= 1);
        assert!((i16::from(restored[5]) - i16::from(source[5])).abs() <= 1);
        assert!((i16::from(restored[6]) - i16::from(source[6])).abs() <= 1);
        assert_eq!(restored[7], source[7]);
    }

    #[test]
    fn overlay_composer_blends_into_anchored_region() {
        let factory: Arc<dyn OverlayRendererFactory> = Arc::new(SolidFactory {
            color: [255, 0, 0, 255],
            interval: Duration::from_secs(1),
        });
        let mut composer = OverlayComposer::new(4, 4, false, factory);
        composer.reconcile(&DisplayOverlayConfig {
            overlays: vec![sample_slot(OverlayPosition::Anchored {
                anchor: Anchor::TopLeft,
                offset_x: 1,
                offset_y: 1,
                width: 2,
                height: 2,
            })],
        });

        let base = vec![0_u8; 4 * 4 * 3];
        let sensors = SystemSnapshot::empty();
        let staging = composer
            .compose_rgb_frame(&base, &sensors, 1, SystemTime::now(), Instant::now())
            .expect("overlay should compose");

        let mut rgb = Vec::new();
        staging.write_into_rgb(&mut rgb);
        assert_eq!(&rgb[0..3], &[0, 0, 0]);
        let offset = ((4 + 1) * 3) as usize;
        assert!(rgb[offset] > 0);
        assert_eq!(rgb[offset + 1], 0);
        assert_eq!(rgb[offset + 2], 0);
    }

    #[test]
    fn runtime_snapshot_marks_disabled_slots() {
        let factory: Arc<dyn OverlayRendererFactory> = Arc::new(SolidFactory {
            color: [255, 0, 0, 255],
            interval: Duration::from_secs(1),
        });
        let mut composer = OverlayComposer::new(4, 4, false, factory);
        let mut slot = sample_slot(OverlayPosition::FullScreen);
        slot.enabled = false;
        composer.reconcile(&DisplayOverlayConfig {
            overlays: vec![slot.clone()],
        });

        let snapshot = composer.runtime_snapshot();
        let runtime = snapshot.slot(slot.id).expect("runtime should exist");
        assert_eq!(runtime.status, OverlaySlotStatus::Disabled);
        assert_eq!(runtime.consecutive_failures, 0);
        assert!(runtime.last_error.is_none());
    }

    #[test]
    fn transient_failures_escalate_to_failed_runtime() {
        struct FlakyRenderer;

        impl OverlayRenderer for FlakyRenderer {
            fn init(&mut self, _target_size: OverlaySize) -> Result<()> {
                Ok(())
            }

            fn resize(&mut self, _target_size: OverlaySize) -> Result<()> {
                Ok(())
            }

            fn render_into(
                &mut self,
                _input: &OverlayInput<'_>,
                _target: &mut OverlayBuffer,
            ) -> std::result::Result<(), OverlayError> {
                Err(OverlayError::Transient(
                    "temporary render failure".to_owned(),
                ))
            }
        }

        struct FlakyFactory;

        impl OverlayRendererFactory for FlakyFactory {
            fn build(
                &self,
                _slot: &OverlaySlot,
                _target_size: OverlaySize,
            ) -> std::result::Result<OverlayRendererBinding, OverlayError> {
                Ok(OverlayRendererBinding {
                    renderer: Box::new(FlakyRenderer),
                    render_interval: Duration::from_millis(16),
                })
            }
        }

        let factory: Arc<dyn OverlayRendererFactory> = Arc::new(FlakyFactory);
        let mut composer = OverlayComposer::new(4, 4, false, factory);
        let slot = sample_slot(OverlayPosition::FullScreen);
        composer.reconcile(&DisplayOverlayConfig {
            overlays: vec![slot.clone()],
        });

        let base = vec![0_u8; 4 * 4 * 3];
        let sensors = SystemSnapshot::empty();
        let start = Instant::now();

        for step in 0_u64..5 {
            let now = start
                .checked_add(Duration::from_secs(step.saturating_mul(31)))
                .expect("instant add should succeed");
            let _ = composer.compose_rgb_frame(&base, &sensors, step + 1, SystemTime::now(), now);
        }

        let snapshot = composer.runtime_snapshot();
        let runtime = snapshot.slot(slot.id).expect("runtime should exist");
        assert_eq!(runtime.status, OverlaySlotStatus::Failed);
        assert_eq!(runtime.consecutive_failures, 5);
        let error = runtime.last_error.as_deref().expect("error should exist");
        assert!(error.contains("disabled after 5 consecutive transient failures"));
    }

    #[test]
    fn renderer_refresh_hint_overrides_binding_interval() {
        struct HintRenderer {
            refresh_after: Duration,
        }

        impl OverlayRenderer for HintRenderer {
            fn init(&mut self, _target_size: OverlaySize) -> Result<()> {
                Ok(())
            }

            fn resize(&mut self, _target_size: OverlaySize) -> Result<()> {
                Ok(())
            }

            fn render_into(
                &mut self,
                _input: &OverlayInput<'_>,
                target: &mut OverlayBuffer,
            ) -> std::result::Result<(), OverlayError> {
                for pixel in target.pixels.chunks_exact_mut(4) {
                    pixel.copy_from_slice(&[255, 0, 0, 255]);
                }
                Ok(())
            }

            fn next_refresh_after(&self) -> Option<Duration> {
                Some(self.refresh_after)
            }
        }

        struct HintFactory {
            refresh_after: Duration,
        }

        impl OverlayRendererFactory for HintFactory {
            fn build(
                &self,
                _slot: &OverlaySlot,
                _target_size: OverlaySize,
            ) -> std::result::Result<OverlayRendererBinding, OverlayError> {
                Ok(OverlayRendererBinding {
                    renderer: Box::new(HintRenderer {
                        refresh_after: self.refresh_after,
                    }),
                    render_interval: Duration::from_secs(60),
                })
            }
        }

        let refresh_after = Duration::from_millis(250);
        let factory: Arc<dyn OverlayRendererFactory> = Arc::new(HintFactory { refresh_after });
        let mut composer = OverlayComposer::new(4, 4, false, factory);
        composer.reconcile(&DisplayOverlayConfig {
            overlays: vec![sample_slot(OverlayPosition::FullScreen)],
        });

        let base = vec![0_u8; 4 * 4 * 3];
        let sensors = SystemSnapshot::empty();
        let start = Instant::now();
        let _ = composer
            .compose_rgb_frame(&base, &sensors, 1, SystemTime::now(), start)
            .expect("overlay should compose");

        let deadline = composer
            .next_refresh_at(start)
            .expect("renderer hint should drive refresh");
        assert_eq!(deadline.duration_since(start), refresh_after);
    }

    #[test]
    fn compose_runtime_change_stays_false_for_cached_frames() {
        struct StableRenderer;

        impl OverlayRenderer for StableRenderer {
            fn init(&mut self, _target_size: OverlaySize) -> Result<()> {
                Ok(())
            }

            fn resize(&mut self, _target_size: OverlaySize) -> Result<()> {
                Ok(())
            }

            fn render_into(
                &mut self,
                _input: &OverlayInput<'_>,
                target: &mut OverlayBuffer,
            ) -> std::result::Result<(), OverlayError> {
                for pixel in target.pixels.chunks_exact_mut(4) {
                    pixel.copy_from_slice(&[255, 0, 0, 255]);
                }
                Ok(())
            }

            fn content_changed(&self, _input: &OverlayInput<'_>) -> bool {
                false
            }
        }

        struct StableFactory;

        impl OverlayRendererFactory for StableFactory {
            fn build(
                &self,
                _slot: &OverlaySlot,
                _target_size: OverlaySize,
            ) -> std::result::Result<OverlayRendererBinding, OverlayError> {
                Ok(OverlayRendererBinding {
                    renderer: Box::new(StableRenderer),
                    render_interval: Duration::from_secs(60),
                })
            }
        }

        let factory: Arc<dyn OverlayRendererFactory> = Arc::new(StableFactory);
        let mut composer = OverlayComposer::new(4, 4, false, factory);
        composer.reconcile(&DisplayOverlayConfig {
            overlays: vec![sample_slot(OverlayPosition::FullScreen)],
        });

        let base = vec![0_u8; 4 * 4 * 3];
        let sensors = SystemSnapshot::empty();
        let start = Instant::now();
        let (_, first_changed) =
            composer.compose_rgb_frame_with_runtime_change(&base, &sensors, 1, SystemTime::now(), start);
        let (_, second_changed) = composer.compose_rgb_frame_with_runtime_change(
            &base,
            &sensors,
            2,
            SystemTime::now(),
            start + Duration::from_millis(16),
        );

        assert!(first_changed);
        assert!(!second_changed);
    }
}
