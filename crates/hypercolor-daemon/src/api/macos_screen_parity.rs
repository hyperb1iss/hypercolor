use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_core::input::screen::{
    CaptureColorSpace, CaptureDynamicRange, CapturePixelFormat, CaptureTransferFunction,
    ScreenBranchPublication,
};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_core::types::canvas::Canvas;
use hypercolor_macos_capture::{
    MacosCaptureError, MacosScreenshotPixelCopy, MacosScreenshotPreferredDynamicRange,
    MacosScreenshotReferenceCapture, MacosScreenshotReferenceSet,
};
use hypercolor_types::event::ZoneColors;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::AppState;
use crate::render_thread::sparkleflinger::{CompositionLayer, CompositionPlan, SparkleFlinger};
use crate::render_thread::{
    MacosScreenParityDiagnosticHandle, MacosScreenParityLiveSnapshot,
    MacosScreenParitySnapshotError,
};

const DIAGNOSTIC_TIMEOUT: Duration = Duration::from_secs(15);
const REFERENCE_WHITE_MIN_CODE_VALUE: u8 = 224;
const REFERENCE_WHITE_MAX_CHANNEL_SPREAD: u8 = 4;
const GAMUT_MIN_CODE_VALUE: u8 = 64;
const GAMUT_MIN_CHANNEL_SPREAD: u8 = 48;
const HIGHLIGHT_MIN_CODE_VALUE: u8 = 192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiagnosticDisposition {
    Unsupported,
    Retry,
    Failed,
}

#[derive(Debug)]
pub(crate) struct MacosScreenParityDiagnosticError {
    disposition: DiagnosticDisposition,
    detail: String,
}

impl MacosScreenParityDiagnosticError {
    fn unsupported(detail: impl Into<String>) -> Self {
        Self {
            disposition: DiagnosticDisposition::Unsupported,
            detail: detail.into(),
        }
    }

    fn retry(detail: impl Into<String>) -> Self {
        Self {
            disposition: DiagnosticDisposition::Retry,
            detail: detail.into(),
        }
    }

    fn failed(detail: impl Into<String>) -> Self {
        Self {
            disposition: DiagnosticDisposition::Failed,
            detail: detail.into(),
        }
    }

    pub(crate) const fn status(&self) -> &'static str {
        match self.disposition {
            DiagnosticDisposition::Unsupported | DiagnosticDisposition::Retry => "warning",
            DiagnosticDisposition::Failed => "fail",
        }
    }

    pub(crate) fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for MacosScreenParityDiagnosticError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for MacosScreenParityDiagnosticError {}

#[derive(Debug, Serialize)]
pub(crate) struct MacosScreenParityReport {
    status: &'static str,
    selection_range: &'static str,
    compared_reference_range: &'static str,
    unsupported: Vec<&'static str>,
    live_pipeline: MacosScreenParityPipelineIdentity,
    layout: MacosScreenParityLayoutIdentity,
    stability: RgbDeltaMetrics,
    surface: MacosScreenParitySurfaceReport,
    final_zone_colors: RgbDeltaMetrics,
    highlight_rolloff: Option<HighlightRolloffMetrics>,
}

impl MacosScreenParityReport {
    pub(crate) fn detail(&self) -> String {
        format!(
            "measured {} pixels and {} LEDs; max surface delta {}, max zone delta {}",
            self.surface.all_pixels.samples,
            self.final_zone_colors.samples,
            self.surface.all_pixels.max_absolute_error,
            self.final_zone_colors.max_absolute_error,
        )
    }
}

#[derive(Debug, Serialize)]
struct MacosScreenParityPipelineIdentity {
    source_id_sha256: String,
    topology_generation: u64,
    capture_session_generation: u64,
    publication_plan_generation: u64,
    descriptor_identity: u64,
    first_native_sequence: u64,
    second_native_sequence: u64,
    source_pixel_format: &'static str,
    source_color_space: &'static str,
    source_transfer_function: &'static str,
    source_dynamic_range: &'static str,
    output_pixel_format: &'static str,
    processing_algorithm_revision: u32,
    tone_map_calibration: MacosScreenParityCalibration,
}

#[derive(Debug, Serialize)]
struct MacosScreenParityCalibration {
    target_white_x: f32,
    target_white_y: f32,
    target_reference_white_nits: f32,
    target_peak_nits: f32,
    exposure_ev: f32,
}

#[derive(Debug, Serialize)]
struct MacosScreenParityLayoutIdentity {
    sha256: String,
    plan_generation: u64,
    canvas_width: u32,
    canvas_height: u32,
    zones: usize,
    leds: usize,
}

#[derive(Debug, Serialize)]
struct MacosScreenParitySurfaceReport {
    width: u32,
    height: u32,
    all_pixels: RgbDeltaMetrics,
    reference_white: ClassifiedRgbDeltaMetrics,
    gamut: ClassifiedRgbDeltaMetrics,
}

#[derive(Debug, Serialize)]
struct ClassifiedRgbDeltaMetrics {
    selector: &'static str,
    metrics: Option<RgbDeltaMetrics>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct RgbDeltaMetrics {
    samples: u64,
    mean_absolute_error: f64,
    root_mean_square_error: f64,
    max_absolute_error: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct HighlightRolloffMetrics {
    samples: u64,
    mean_standard_luma: f64,
    mean_high_luma: f64,
    mean_high_minus_standard_luma: f64,
    mean_absolute_luma_difference: f64,
    max_absolute_luma_difference: f64,
}

struct RenderedReferences {
    selection_range: &'static str,
    compared_reference_range: &'static str,
    unsupported: Vec<&'static str>,
    reference: MacosScreenshotPixelCopy,
    highlight_rolloff: Option<HighlightRolloffMetrics>,
}

fn reference_zones(
    reference: &MacosScreenshotPixelCopy,
    canvas_width: u32,
    canvas_height: u32,
    spatial_engine: &SpatialEngine,
) -> anyhow::Result<Vec<ZoneColors>> {
    let canvas = Canvas::try_from_rgba(
        &reference.rgba8,
        reference.extent.width,
        reference.extent.height,
    )?;
    let composed = SparkleFlinger::cpu().compose(
        CompositionPlan::single(
            canvas_width,
            canvas_height,
            CompositionLayer::replace_canvas(canvas),
        )
        .with_cpu_replay_cacheable(false),
    );
    let canvas = composed
        .sampling_canvas
        .ok_or_else(|| anyhow::anyhow!("the CPU reference compositor produced no canvas"))?;
    Ok(spatial_engine.try_sample(&canvas)?)
}

pub(crate) async fn run_macos_screen_parity(
    state: &AppState,
) -> Result<MacosScreenParityReport, MacosScreenParityDiagnosticError> {
    let deadline = Instant::now() + DIAGNOSTIC_TIMEOUT;
    let diagnostics = state
        .macos_screen_parity_diagnostics
        .clone()
        .ok_or_else(|| {
            MacosScreenParityDiagnosticError::unsupported(
                "macOS screen parity requires the active Metal render thread",
            )
        })?;
    let screenshot_action = {
        let input = state.input_manager.lock().await;
        input.diagnostic_artifact_action()
    };
    let screenshot_action = screenshot_action.ok_or_else(|| {
        MacosScreenParityDiagnosticError::unsupported(
            "no active macOS screen capture exposes screenshot references",
        )
    })?;

    let first = capture_live_snapshot(&diagnostics, deadline).await?;
    let first_layout = first.spatial_engine.layout();
    let first_layout_generation = first.spatial_engine.plan_generation();
    if first.spatial_engine.sampling_plan().is_empty() {
        return Err(MacosScreenParityDiagnosticError::unsupported(
            "the active spatial layout has no LED sampling plan",
        ));
    }
    let layout_hash = layout_sha256(first_layout.as_ref())?;

    let screenshot_rx = screenshot_action()
        .map_err(|error| map_screenshot_action_error(&error))?
        .downcast::<
            std::sync::mpsc::Receiver<
                Result<MacosScreenshotReferenceCapture, MacosCaptureError>,
            >,
        >()
        .map(|receiver| *receiver)
        .map_err(|_| {
            MacosScreenParityDiagnosticError::failed(
                "the screen source returned an unsupported diagnostic artifact",
            )
        })?;
    let screenshot_capture = receive_screenshot_capture(screenshot_rx, deadline).await?;
    validate_capture_identity(&first.publication, &screenshot_capture)?;

    let second = capture_live_snapshot(&diagnostics, deadline).await?;
    validate_live_identity(&first, &second)?;
    if second.spatial_engine.plan_generation() != first_layout_generation
        || layout_sha256(second.spatial_engine.layout().as_ref())? != layout_hash
    {
        return Err(MacosScreenParityDiagnosticError::retry(
            "the active spatial layout changed during the parity transaction; retry",
        ));
    }
    let stability = require_static_live_content(&first, &second)?;

    let current_spatial = state.spatial_engine.read().await;
    let current_layout = current_spatial.layout();
    if current_spatial.plan_generation() != first_layout_generation
        || !Arc::ptr_eq(&current_layout, &first_layout)
    {
        return Err(MacosScreenParityDiagnosticError::retry(
            "the spatial layout changed during the parity transaction; retry",
        ));
    }
    build_report(first, second, screenshot_capture, layout_hash, stability)
}

async fn capture_live_snapshot(
    diagnostics: &MacosScreenParityDiagnosticHandle,
    deadline: Instant,
) -> Result<MacosScreenParityLiveSnapshot, MacosScreenParityDiagnosticError> {
    tokio::time::timeout(remaining(deadline)?, diagnostics.snapshot())
        .await
        .map_err(|_| {
            MacosScreenParityDiagnosticError::retry(
                "the active render thread did not service the parity request; retry",
            )
        })?
        .map_err(map_snapshot_error)
}

async fn receive_screenshot_capture(
    receiver: std::sync::mpsc::Receiver<Result<MacosScreenshotReferenceCapture, MacosCaptureError>>,
    deadline: Instant,
) -> Result<MacosScreenshotReferenceCapture, MacosScreenParityDiagnosticError> {
    let timeout = remaining(deadline)?;
    tokio::task::spawn_blocking(move || receiver.recv_timeout(timeout))
        .await
        .map_err(|_| {
            MacosScreenParityDiagnosticError::failed(
                "the screenshot reference receiver task failed",
            )
        })?
        .map_err(|_| {
            MacosScreenParityDiagnosticError::retry(
                "the screenshot reference transaction timed out; retry",
            )
        })?
        .map_err(|error| map_screenshot_error(&error))
}

fn map_snapshot_error(error: MacosScreenParitySnapshotError) -> MacosScreenParityDiagnosticError {
    match error {
        MacosScreenParitySnapshotError::NoActiveScreenPublication => {
            MacosScreenParityDiagnosticError::unsupported(
                "the active renderer has no live screen publication",
            )
        }
        MacosScreenParitySnapshotError::PublicationIdentityChanged => {
            MacosScreenParityDiagnosticError::retry(
                "the active publication identity changed during the parity request; retry",
            )
        }
        MacosScreenParitySnapshotError::SamplingUnavailable => {
            MacosScreenParityDiagnosticError::retry(
                "the active GPU sampler could not accept the parity request; retry",
            )
        }
        MacosScreenParitySnapshotError::RendererStopped => {
            MacosScreenParityDiagnosticError::failed(
                "the active render thread stopped during the parity transaction",
            )
        }
        MacosScreenParitySnapshotError::UnsupportedOutputFormat => {
            MacosScreenParityDiagnosticError::unsupported(
                "the active screen branch does not publish RGBA8 output",
            )
        }
        MacosScreenParitySnapshotError::NativeReductionFailed
        | MacosScreenParitySnapshotError::SurfaceReadbackFailed
        | MacosScreenParitySnapshotError::SpatialSamplingFailed => {
            MacosScreenParityDiagnosticError::failed(error.to_string())
        }
    }
}

fn map_screenshot_action_error(error: &anyhow::Error) -> MacosScreenParityDiagnosticError {
    if let Some(error) = error.downcast_ref::<MacosCaptureError>() {
        return map_screenshot_error(error);
    }
    MacosScreenParityDiagnosticError::failed(
        "the screenshot reference transaction could not be started",
    )
}

fn remaining(deadline: Instant) -> Result<Duration, MacosScreenParityDiagnosticError> {
    deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| {
            MacosScreenParityDiagnosticError::retry("the parity transaction timed out; retry")
        })
}

fn layout_sha256(
    layout: &hypercolor_types::spatial::SpatialLayout,
) -> Result<String, MacosScreenParityDiagnosticError> {
    serde_json::to_vec(layout)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|_| {
            MacosScreenParityDiagnosticError::failed(
                "the active spatial layout could not be serialized",
            )
        })
}

fn map_screenshot_error(error: &MacosCaptureError) -> MacosScreenParityDiagnosticError {
    match error {
        MacosCaptureError::ScreenshotCapabilityPending => MacosScreenParityDiagnosticError::retry(
            "Tahoe screenshot capability is pending the first complete frame; retry",
        ),
        MacosCaptureError::ScreenshotSelectionChanged => MacosScreenParityDiagnosticError::retry(
            "the selected screen source changed during the parity transaction; retry",
        ),
        MacosCaptureError::ScreenCapturePermissionRequired => {
            MacosScreenParityDiagnosticError::unsupported(
                "Screen Recording permission requires an explicit user action",
            )
        }
        MacosCaptureError::TahoePlatformDefect(_) => MacosScreenParityDiagnosticError::unsupported(
            "the active Tahoe runtime lacks required screenshot reference facilities",
        ),
        _ => MacosScreenParityDiagnosticError::failed(
            "the native screenshot reference transaction failed",
        ),
    }
}

fn validate_capture_identity(
    publication: &ScreenBranchPublication,
    capture: &MacosScreenshotReferenceCapture,
) -> Result<(), MacosScreenParityDiagnosticError> {
    let epoch = publication.source_epoch();
    if capture.source_id() != epoch.source_id.as_str()
        || capture.capture_session_generation() != epoch.session_generation
    {
        return Err(MacosScreenParityDiagnosticError::retry(
            "the screenshot and live publication came from different capture identities; retry",
        ));
    }
    Ok(())
}

fn validate_live_identity(
    first: &MacosScreenParityLiveSnapshot,
    second: &MacosScreenParityLiveSnapshot,
) -> Result<(), MacosScreenParityDiagnosticError> {
    if second.publication.native_sequence() <= first.publication.native_sequence()
        || second.publication.plan_generation() != first.publication.plan_generation()
        || second.publication.descriptor_identity() != first.publication.descriptor_identity()
        || second.publication.source_epoch() != first.publication.source_epoch()
        || second.descriptor != first.descriptor
    {
        return Err(MacosScreenParityDiagnosticError::retry(
            "the live publication identity changed or did not advance during the parity transaction; retry",
        ));
    }
    Ok(())
}

fn require_static_live_content(
    first: &MacosScreenParityLiveSnapshot,
    second: &MacosScreenParityLiveSnapshot,
) -> Result<RgbDeltaMetrics, MacosScreenParityDiagnosticError> {
    if first.width != second.width
        || first.height != second.height
        || first.rgba8 != second.rgba8
        || first.zones != second.zones
    {
        return Err(MacosScreenParityDiagnosticError::retry(
            "screen content changed during the parity transaction; show a static calibration image and retry",
        ));
    }
    rgb_delta_metrics(&first.rgba8, &second.rgba8, |_| true)
        .map_err(MacosScreenParityDiagnosticError::failed)?
        .ok_or_else(|| MacosScreenParityDiagnosticError::failed("the live surface was empty"))
}

fn build_report(
    first: MacosScreenParityLiveSnapshot,
    live: MacosScreenParityLiveSnapshot,
    screenshot_capture: MacosScreenshotReferenceCapture,
    layout_sha256: String,
    stability: RgbDeltaMetrics,
) -> Result<MacosScreenParityReport, MacosScreenParityDiagnosticError> {
    let first_publication = &first.publication;
    let second_publication = &live.publication;
    let live_descriptor = &first.descriptor;
    if live_descriptor.source_epoch() != first_publication.source_epoch()
        || live_descriptor.processing_profile().target_pixel_format() != CapturePixelFormat::Rgba8
    {
        return Err(MacosScreenParityDiagnosticError::retry(
            "the diagnostic branch descriptor no longer matches the live publication; retry",
        ));
    }
    let resolved = first.spatial_engine.layout();
    let live_dynamic_range = live_descriptor
        .source_colorimetry()
        .dynamic_range()
        .ok_or_else(|| {
            MacosScreenParityDiagnosticError::failed(
                "the live publication omitted its dynamic range",
            )
        })?;
    let references = render_references(screenshot_capture, live_dynamic_range)?;
    ensure_matching_extent(&live, &references.reference)?;
    let reference_zones = reference_zones(
        &references.reference,
        resolved.canvas_width,
        resolved.canvas_height,
        &first.spatial_engine,
    )
    .map_err(|_| {
        MacosScreenParityDiagnosticError::failed(
            "the Core Graphics reference could not be sampled into final zone colors",
        )
    })?;
    let final_zone_colors = zone_delta_metrics(&reference_zones, &live.zones)
        .map_err(MacosScreenParityDiagnosticError::failed)?;
    let all_pixels = rgb_delta_metrics(&references.reference.rgba8, &live.rgba8, |_| true)
        .map_err(MacosScreenParityDiagnosticError::failed)?
        .ok_or_else(|| MacosScreenParityDiagnosticError::failed("the parity surface was empty"))?;
    let reference_white = rgb_delta_metrics(&references.reference.rgba8, &live.rgba8, |rgb| {
        rgb.iter().copied().max().unwrap_or(0) >= REFERENCE_WHITE_MIN_CODE_VALUE
            && channel_spread(rgb) <= REFERENCE_WHITE_MAX_CHANNEL_SPREAD
    })
    .map_err(MacosScreenParityDiagnosticError::failed)?;
    let gamut = rgb_delta_metrics(&references.reference.rgba8, &live.rgba8, |rgb| {
        rgb.iter().copied().max().unwrap_or(0) >= GAMUT_MIN_CODE_VALUE
            && channel_spread(rgb) >= GAMUT_MIN_CHANNEL_SPREAD
    })
    .map_err(MacosScreenParityDiagnosticError::failed)?;
    let calibration = live_descriptor.processing_profile().led_tone_map();
    let live_epoch = first_publication.source_epoch();
    let leds = live.zones.iter().map(|zone| zone.colors.len()).sum();

    Ok(MacosScreenParityReport {
        status: "measured",
        selection_range: references.selection_range,
        compared_reference_range: references.compared_reference_range,
        unsupported: references.unsupported,
        live_pipeline: MacosScreenParityPipelineIdentity {
            source_id_sha256: sha256_hex(live_epoch.source_id.as_str().as_bytes()),
            topology_generation: live_epoch.topology_generation,
            capture_session_generation: live_epoch.session_generation,
            publication_plan_generation: first_publication.plan_generation().get(),
            descriptor_identity: first_publication.descriptor_identity().get(),
            first_native_sequence: first_publication.native_sequence().get(),
            second_native_sequence: second_publication.native_sequence().get(),
            source_pixel_format: pixel_format_name(live_descriptor.source_pixel_format()),
            source_color_space: color_space_name(
                live_descriptor.source_colorimetry().color_space(),
            ),
            source_transfer_function: transfer_function_name(
                live_descriptor.source_colorimetry().transfer_function(),
            ),
            source_dynamic_range: dynamic_range_name(live_dynamic_range),
            output_pixel_format: pixel_format_name(
                live_descriptor.processing_profile().target_pixel_format(),
            ),
            processing_algorithm_revision: live_descriptor
                .processing_profile()
                .algorithm_revision()
                .get(),
            tone_map_calibration: MacosScreenParityCalibration {
                target_white_x: calibration.target_white_x(),
                target_white_y: calibration.target_white_y(),
                target_reference_white_nits: calibration.target_reference_white_nits(),
                target_peak_nits: calibration.target_peak_nits(),
                exposure_ev: calibration.exposure_ev(),
            },
        },
        layout: MacosScreenParityLayoutIdentity {
            sha256: layout_sha256,
            plan_generation: first.spatial_engine.plan_generation(),
            canvas_width: resolved.canvas_width,
            canvas_height: resolved.canvas_height,
            zones: live.zones.len(),
            leds,
        },
        stability,
        surface: MacosScreenParitySurfaceReport {
            width: live.width,
            height: live.height,
            all_pixels,
            reference_white: ClassifiedRgbDeltaMetrics {
                selector: "reference RGB max >= 224 and channel spread <= 4",
                metrics: reference_white,
            },
            gamut: ClassifiedRgbDeltaMetrics {
                selector: "reference RGB max >= 64 and channel spread >= 48",
                metrics: gamut,
            },
        },
        final_zone_colors,
        highlight_rolloff: references.highlight_rolloff,
    })
}

fn render_references(
    capture: MacosScreenshotReferenceCapture,
    live_dynamic_range: CaptureDynamicRange,
) -> Result<RenderedReferences, MacosScreenParityDiagnosticError> {
    match capture.into_references() {
        MacosScreenshotReferenceSet::Sdr { image } => {
            if live_dynamic_range != CaptureDynamicRange::Standard {
                return Err(MacosScreenParityDiagnosticError::retry(
                    "an SDR-only screenshot selection produced an HDR live publication; retry after capture reconfiguration",
                ));
            }
            let reference = image
                .copy_reference_rgba8(MacosScreenshotPreferredDynamicRange::Standard)
                .map_err(|error| map_screenshot_error(&error))?;
            Ok(RenderedReferences {
                selection_range: "sdr_only",
                compared_reference_range: "standard",
                unsupported: vec!["hdr_reference", "paired_highlight_rolloff"],
                reference,
                highlight_rolloff: None,
            })
        }
        MacosScreenshotReferenceSet::Paired { sdr, hdr } => {
            let standard = sdr
                .copy_reference_rgba8(MacosScreenshotPreferredDynamicRange::Standard)
                .map_err(|error| map_screenshot_error(&error))?;
            let high = hdr
                .copy_reference_rgba8(MacosScreenshotPreferredDynamicRange::High)
                .map_err(|error| map_screenshot_error(&error))?;
            ensure_reference_pair_extent(&standard, &high)?;
            let highlight_rolloff = highlight_rolloff_metrics(&standard.rgba8, &high.rgba8)
                .map_err(MacosScreenParityDiagnosticError::failed)?;
            match live_dynamic_range {
                CaptureDynamicRange::Standard => Ok(RenderedReferences {
                    selection_range: "paired_sdr_hdr",
                    compared_reference_range: "standard",
                    unsupported: Vec::new(),
                    reference: standard,
                    highlight_rolloff,
                }),
                CaptureDynamicRange::High => Ok(RenderedReferences {
                    selection_range: "paired_sdr_hdr",
                    compared_reference_range: "high",
                    unsupported: Vec::new(),
                    reference: high,
                    highlight_rolloff,
                }),
            }
        }
    }
}

fn ensure_matching_extent(
    live: &MacosScreenParityLiveSnapshot,
    reference: &MacosScreenshotPixelCopy,
) -> Result<(), MacosScreenParityDiagnosticError> {
    if live.width != reference.extent.width || live.height != reference.extent.height {
        return Err(MacosScreenParityDiagnosticError::retry(
            "the live publication and Core Graphics reference extents differ; retry after capture settles",
        ));
    }
    Ok(())
}

fn ensure_reference_pair_extent(
    standard: &MacosScreenshotPixelCopy,
    high: &MacosScreenshotPixelCopy,
) -> Result<(), MacosScreenParityDiagnosticError> {
    if standard.extent != high.extent {
        return Err(MacosScreenParityDiagnosticError::retry(
            "the paired Core Graphics reference extents differ; retry",
        ));
    }
    Ok(())
}

fn rgb_delta_metrics(
    reference: &[u8],
    actual: &[u8],
    mut include: impl FnMut([u8; 3]) -> bool,
) -> Result<Option<RgbDeltaMetrics>, &'static str> {
    if reference.len() != actual.len() || !reference.len().is_multiple_of(4) {
        return Err("RGBA parity buffers have incompatible lengths");
    }
    let mut samples = 0_u64;
    let mut absolute_sum = 0_u64;
    let mut square_sum = 0_u64;
    let mut maximum = 0_u8;
    for (reference, actual) in reference.chunks_exact(4).zip(actual.chunks_exact(4)) {
        let rgb = [reference[0], reference[1], reference[2]];
        if !include(rgb) {
            continue;
        }
        samples = samples.saturating_add(1);
        for channel in 0..3 {
            let delta = reference[channel].abs_diff(actual[channel]);
            absolute_sum = absolute_sum.saturating_add(u64::from(delta));
            square_sum = square_sum.saturating_add(u64::from(delta) * u64::from(delta));
            maximum = maximum.max(delta);
        }
    }
    if samples == 0 {
        return Ok(None);
    }
    let channel_samples = (samples as f64) * 3.0;
    Ok(Some(RgbDeltaMetrics {
        samples,
        mean_absolute_error: (absolute_sum as f64) / channel_samples,
        root_mean_square_error: ((square_sum as f64) / channel_samples).sqrt(),
        max_absolute_error: maximum,
    }))
}

fn zone_delta_metrics(
    reference: &[ZoneColors],
    actual: &[ZoneColors],
) -> Result<RgbDeltaMetrics, &'static str> {
    if reference.len() != actual.len() {
        return Err("reference and live zone counts differ");
    }
    let mut samples = 0_u64;
    let mut absolute_sum = 0_u64;
    let mut square_sum = 0_u64;
    let mut maximum = 0_u8;
    for (reference, actual) in reference.iter().zip(actual) {
        if reference.zone_id != actual.zone_id || reference.colors.len() != actual.colors.len() {
            return Err("reference and live zone identities differ");
        }
        for (reference, actual) in reference.colors.iter().zip(&actual.colors) {
            samples = samples.saturating_add(1);
            for channel in 0..3 {
                let delta = reference[channel].abs_diff(actual[channel]);
                absolute_sum = absolute_sum.saturating_add(u64::from(delta));
                square_sum = square_sum.saturating_add(u64::from(delta) * u64::from(delta));
                maximum = maximum.max(delta);
            }
        }
    }
    if samples == 0 {
        return Err("the active layout produced no final LED colors");
    }
    let channel_samples = (samples as f64) * 3.0;
    Ok(RgbDeltaMetrics {
        samples,
        mean_absolute_error: (absolute_sum as f64) / channel_samples,
        root_mean_square_error: ((square_sum as f64) / channel_samples).sqrt(),
        max_absolute_error: maximum,
    })
}

fn highlight_rolloff_metrics(
    standard: &[u8],
    high: &[u8],
) -> Result<Option<HighlightRolloffMetrics>, &'static str> {
    if standard.len() != high.len() || !standard.len().is_multiple_of(4) {
        return Err("paired reference buffers have incompatible lengths");
    }
    let mut samples = 0_u64;
    let mut standard_sum = 0.0;
    let mut high_sum = 0.0;
    let mut signed_delta_sum = 0.0;
    let mut absolute_delta_sum = 0.0;
    let mut maximum_delta = 0.0_f64;
    for (standard, high) in standard.chunks_exact(4).zip(high.chunks_exact(4)) {
        if standard[..3]
            .iter()
            .chain(&high[..3])
            .copied()
            .max()
            .unwrap_or(0)
            < HIGHLIGHT_MIN_CODE_VALUE
        {
            continue;
        }
        let standard_luma = encoded_luma(standard);
        let high_luma = encoded_luma(high);
        let delta = high_luma - standard_luma;
        samples = samples.saturating_add(1);
        standard_sum += standard_luma;
        high_sum += high_luma;
        signed_delta_sum += delta;
        absolute_delta_sum += delta.abs();
        maximum_delta = maximum_delta.max(delta.abs());
    }
    if samples == 0 {
        return Ok(None);
    }
    let samples_f64 = samples as f64;
    Ok(Some(HighlightRolloffMetrics {
        samples,
        mean_standard_luma: standard_sum / samples_f64,
        mean_high_luma: high_sum / samples_f64,
        mean_high_minus_standard_luma: signed_delta_sum / samples_f64,
        mean_absolute_luma_difference: absolute_delta_sum / samples_f64,
        max_absolute_luma_difference: maximum_delta,
    }))
}

fn encoded_luma(rgba: &[u8]) -> f64 {
    (0.2126 * f64::from(rgba[0]) + 0.7152 * f64::from(rgba[1]) + 0.0722 * f64::from(rgba[2]))
        / 255.0
}

fn channel_spread(rgb: [u8; 3]) -> u8 {
    rgb.iter().copied().max().unwrap_or(0) - rgb.iter().copied().min().unwrap_or(0)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut output, "{byte:02x}").expect("writing into a String cannot fail");
    }
    output
}

const fn pixel_format_name(format: CapturePixelFormat) -> &'static str {
    match format {
        CapturePixelFormat::Rgba8 => "rgba8_unorm",
        CapturePixelFormat::Bgra8 => "bgra8_unorm",
        CapturePixelFormat::Argb2101010 => "argb2101010",
        CapturePixelFormat::Rgba16Float => "rgba16_float",
        CapturePixelFormat::Yuv420VideoRange => "yuv420_video_range",
        CapturePixelFormat::Yuv420FullRange => "yuv420_full_range",
        CapturePixelFormat::Yuv44410BiPlanar => "yuv44410_biplanar",
    }
}

const fn color_space_name(color_space: CaptureColorSpace) -> &'static str {
    match color_space {
        CaptureColorSpace::Srgb => "srgb",
        CaptureColorSpace::DisplayP3 => "display_p3",
        CaptureColorSpace::Rec2020 => "rec2020",
        CaptureColorSpace::Unknown => "unknown",
    }
}

const fn transfer_function_name(transfer: CaptureTransferFunction) -> &'static str {
    match transfer {
        CaptureTransferFunction::Srgb => "srgb",
        CaptureTransferFunction::Linear => "linear",
        CaptureTransferFunction::Rec709 => "rec709",
        CaptureTransferFunction::Rec2020 => "rec2020",
        CaptureTransferFunction::Pq => "pq",
        CaptureTransferFunction::Hlg => "hlg",
        CaptureTransferFunction::Unknown => "unknown",
    }
}

const fn dynamic_range_name(range: CaptureDynamicRange) -> &'static str {
    match range {
        CaptureDynamicRange::Standard => "standard",
        CaptureDynamicRange::High => "high",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_metrics_select_reference_white_and_gamut_independently() {
        let reference = [240, 240, 240, 255, 240, 32, 16, 255, 24, 24, 24, 255];
        let actual = [238, 241, 240, 255, 230, 34, 20, 255, 24, 24, 24, 255];

        let white = rgb_delta_metrics(&reference, &actual, |rgb| {
            rgb.iter().copied().max().unwrap_or(0) >= REFERENCE_WHITE_MIN_CODE_VALUE
                && channel_spread(rgb) <= REFERENCE_WHITE_MAX_CHANNEL_SPREAD
        })
        .expect("white metrics")
        .expect("one white sample");
        let gamut = rgb_delta_metrics(&reference, &actual, |rgb| {
            rgb.iter().copied().max().unwrap_or(0) >= GAMUT_MIN_CODE_VALUE
                && channel_spread(rgb) >= GAMUT_MIN_CHANNEL_SPREAD
        })
        .expect("gamut metrics")
        .expect("one gamut sample");

        assert_eq!(white.samples, 1);
        assert_eq!(white.max_absolute_error, 2);
        assert_eq!(gamut.samples, 1);
        assert_eq!(gamut.max_absolute_error, 10);
    }

    #[test]
    fn zone_metrics_reject_identity_drift() {
        let reference = [ZoneColors {
            zone_id: "zone-a".to_owned(),
            colors: vec![[1, 2, 3]],
        }];
        let actual = [ZoneColors {
            zone_id: "zone-b".to_owned(),
            colors: vec![[1, 2, 3]],
        }];

        assert_eq!(
            zone_delta_metrics(&reference, &actual),
            Err("reference and live zone identities differ")
        );
    }

    #[test]
    fn paired_highlight_metrics_keep_signed_rolloff_direction() {
        let standard = [192, 192, 192, 255, 32, 32, 32, 255];
        let high = [224, 224, 224, 255, 32, 32, 32, 255];

        let metrics = highlight_rolloff_metrics(&standard, &high)
            .expect("highlight metrics")
            .expect("one highlight sample");

        assert_eq!(metrics.samples, 1);
        assert!(metrics.mean_high_minus_standard_luma > 0.0);
        assert_eq!(
            metrics.mean_absolute_luma_difference,
            metrics.mean_high_minus_standard_luma
        );
    }

    #[test]
    fn pending_screenshot_action_remains_retryable() {
        let error = anyhow::Error::new(MacosCaptureError::ScreenshotCapabilityPending);

        let mapped = map_screenshot_action_error(&error);

        assert_eq!(mapped.status(), "warning");
        assert!(mapped.detail().contains("pending the first complete frame"));
    }

    #[test]
    fn saturated_live_sampler_remains_retryable() {
        let mapped = map_snapshot_error(MacosScreenParitySnapshotError::SamplingUnavailable);

        assert_eq!(mapped.status(), "warning");
        assert!(mapped.detail().contains("retry"));
    }
}
