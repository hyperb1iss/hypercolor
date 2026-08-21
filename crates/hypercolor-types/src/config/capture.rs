use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::defaults;

// ─── Screen Capture ──────────────────────────────────────────────────────────

/// Native acquisition cadence for screen capture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureCadenceMode {
    /// Acquire at `capture_fps`.
    #[default]
    Fixed,
    /// Allow the native backend to acquire at the display refresh cadence.
    NativeRefresh,
}

/// Screen capture settings for ambient lighting effects.
///
/// The capture source is chosen interactively through the desktop portal
/// picker; `restore_token` persists that choice across daemon restarts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureConfig {
    #[serde(default = "defaults::capture_enabled")]
    pub enabled: bool,

    #[serde(default = "defaults::capture_source")]
    pub source: String,

    #[serde(default = "defaults::capture_fps")]
    pub capture_fps: u32,

    #[serde(default)]
    pub cadence: CaptureCadenceMode,

    /// Sector grid columns for ambilight zone sampling.
    #[serde(default = "defaults::capture_grid_cols")]
    pub grid_cols: u32,

    /// Sector grid rows for ambilight zone sampling.
    #[serde(default = "defaults::capture_grid_rows")]
    pub grid_rows: u32,

    /// Process-memory byte budget shared by analysis and screen publications.
    ///
    /// When omitted, the daemon snapshots currently available host memory
    /// during startup. The analyzer reserves its peak first and publication
    /// plans consume the remainder. Dimensions remain unconstrained; checked
    /// memory and compute admission determine whether a configuration fits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication_memory_bytes: Option<u64>,

    /// Temporal smoothing factor (0.0 = frozen, 1.0 = raw).
    #[serde(default = "defaults::capture_smoothing")]
    pub smoothing: f32,

    /// Frame-difference threshold that bypasses smoothing on scene cuts.
    #[serde(default = "defaults::capture_scene_cut_threshold")]
    pub scene_cut_threshold: f32,

    /// Auto-detect and crop black letterbox/pillarbox bars.
    ///
    /// Off by default: ambient lighting almost always mirrors a desktop, not
    /// a letterboxed film, and dark desktop content trips the detector into
    /// cropping real picture away. Turn it on when mirroring video that
    /// genuinely has bars.
    #[serde(default)]
    pub letterbox: bool,

    /// Luminance threshold for letterbox detection (0.0 - 1.0).
    #[serde(default = "defaults::capture_letterbox_threshold")]
    pub letterbox_threshold: f32,

    /// Saturation boost applied to zone colors (1.0 = neutral).
    #[serde(default = "defaults::unit_scale")]
    pub saturation: f32,

    /// Brightness multiplier applied to zone colors (1.0 = neutral).
    #[serde(default = "defaults::unit_scale")]
    pub brightness: f32,

    /// Gamma shaping applied to zone colors (1.0 = neutral, >1 darkens mids).
    #[serde(default = "defaults::unit_scale")]
    pub gamma: f32,

    /// Target LED white-point x coordinate in CIE xy chromaticity space.
    #[serde(default = "defaults::capture_target_led_white_x")]
    pub target_led_white_x: f32,

    /// Target LED white-point y coordinate in CIE xy chromaticity space.
    #[serde(default = "defaults::capture_target_led_white_y")]
    pub target_led_white_y: f32,

    /// Target LED reference white in nits for HDR tone mapping.
    #[serde(default = "defaults::capture_target_led_reference_white_nits")]
    pub target_led_reference_white_nits: f32,

    /// Calibrated target LED peak in nits for HDR tone mapping.
    #[serde(default = "defaults::capture_target_led_peak_nits")]
    pub target_led_peak_nits: f32,

    /// User exposure adjustment in exposure-value stops.
    #[serde(default = "defaults::capture_exposure_ev")]
    pub exposure_ev: f32,

    /// XDG portal restore token so the picked source survives restarts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_token: Option<String>,
}

/// Native capture implementation selected by the current target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturePlatform {
    /// DXGI Desktop Duplication.
    WindowsDesktopDuplication,
    /// XDG desktop portal plus PipeWire.
    LinuxPipeWire,
    /// ScreenCaptureKit with the system content picker.
    MacosScreenCaptureKit,
    /// No native screen-capture implementation is available.
    Unsupported,
}

impl CapturePlatform {
    /// Capture platform compiled into this target.
    #[must_use]
    pub const fn current() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::WindowsDesktopDuplication
        }
        #[cfg(target_os = "linux")]
        {
            Self::LinuxPipeWire
        }
        #[cfg(target_os = "macos")]
        {
            Self::MacosScreenCaptureKit
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
        {
            Self::Unsupported
        }
    }
}

/// Invalid screen-capture configuration rejected before persistence or startup.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CaptureConfigValidationError {
    /// The target has no native capture backend.
    #[error("screen capture is not supported on this platform")]
    UnsupportedPlatform,
    /// Capture cadence must be non-zero.
    #[error("capture.capture_fps must be non-zero, got {value}")]
    CaptureFps {
        /// Rejected value.
        value: u32,
    },
    /// A grid dimension is empty.
    #[error("capture.{field} must be non-zero, got {value}")]
    GridDimension {
        /// Config field name.
        field: &'static str,
        /// Rejected value.
        value: u32,
    },
    /// An explicit publication-memory budget is empty.
    #[error("capture.publication_memory_bytes must be non-zero, got {value}")]
    PublicationMemoryBudget {
        /// Rejected byte budget.
        value: u64,
    },
    /// A floating-point setting is non-finite or outside its semantic range.
    #[error("capture.{field} must be finite and in {min}..={max}, got {value}")]
    FloatRange {
        /// Config field name.
        field: &'static str,
        /// Inclusive lower bound.
        min: f32,
        /// Inclusive upper bound.
        max: f32,
        /// Rejected value.
        value: f32,
    },
    /// The target LED white point lies outside the CIE xy triangle.
    #[error(
        "capture target LED white point must be finite with x > 0, y > 0, and x + y < 1, got ({x}, {y})"
    )]
    WhitePointChromaticity {
        /// Rejected CIE xy x coordinate.
        x: f32,
        /// Rejected CIE xy y coordinate.
        y: f32,
    },
    /// Target peak does not leave any headroom above reference white.
    #[error(
        "capture.target_led_peak_nits must be greater than target_led_reference_white_nits ({reference}), got {peak}"
    )]
    PeakNotAboveReference {
        /// Configured target reference white in nits.
        reference: f32,
        /// Rejected target peak in nits.
        peak: f32,
    },
    /// The selected source cannot be represented by the native backend.
    #[error("capture.source is invalid for {platform}: {reason}")]
    Source {
        /// Backend accepting the source string.
        platform: &'static str,
        /// Specific validation failure.
        reason: &'static str,
    },
}

impl CaptureConfig {
    /// Validate every capture setting against the backend compiled for this target.
    ///
    /// # Errors
    ///
    /// Returns the first unsupported or out-of-range setting.
    pub fn validate(&self) -> Result<(), CaptureConfigValidationError> {
        self.validate_for_platform(CapturePlatform::current())
    }

    /// Validate against an explicit backend for cross-platform contract tests.
    ///
    /// # Errors
    ///
    /// Returns the first unsupported or out-of-range setting.
    pub fn validate_for_platform(
        &self,
        platform: CapturePlatform,
    ) -> Result<(), CaptureConfigValidationError> {
        if self.capture_fps == 0 {
            return Err(CaptureConfigValidationError::CaptureFps {
                value: self.capture_fps,
            });
        }
        validate_grid_dimension("grid_cols", self.grid_cols)?;
        validate_grid_dimension("grid_rows", self.grid_rows)?;
        if self.publication_memory_bytes == Some(0) {
            return Err(CaptureConfigValidationError::PublicationMemoryBudget { value: 0 });
        }
        validate_capture_float("smoothing", self.smoothing, 0.0, 1.0)?;
        validate_capture_float("scene_cut_threshold", self.scene_cut_threshold, 0.0, 765.0)?;
        validate_capture_float("letterbox_threshold", self.letterbox_threshold, 0.0, 1.0)?;
        validate_capture_float("saturation", self.saturation, 0.0, 4.0)?;
        validate_capture_float("brightness", self.brightness, 0.0, 4.0)?;
        validate_capture_float("gamma", self.gamma, 0.2, 5.0)?;
        if !self.target_led_white_x.is_finite()
            || !self.target_led_white_y.is_finite()
            || self.target_led_white_x <= 0.0
            || self.target_led_white_y <= 0.0
            || self.target_led_white_x + self.target_led_white_y >= 1.0
        {
            return Err(CaptureConfigValidationError::WhitePointChromaticity {
                x: self.target_led_white_x,
                y: self.target_led_white_y,
            });
        }
        validate_capture_float(
            "target_led_reference_white_nits",
            self.target_led_reference_white_nits,
            1.0,
            5_000.0,
        )?;
        validate_capture_float(
            "target_led_peak_nits",
            self.target_led_peak_nits,
            1.0,
            10_000.0,
        )?;
        if self.target_led_peak_nits <= self.target_led_reference_white_nits {
            return Err(CaptureConfigValidationError::PeakNotAboveReference {
                reference: self.target_led_reference_white_nits,
                peak: self.target_led_peak_nits,
            });
        }
        validate_capture_float("exposure_ev", self.exposure_ev, -8.0, 8.0)?;
        validate_capture_source(platform, &self.source, self.enabled)?;
        if matches!(platform, CapturePlatform::Unsupported) && self.enabled {
            return Err(CaptureConfigValidationError::UnsupportedPlatform);
        }
        Ok(())
    }
}

fn validate_grid_dimension(
    field: &'static str,
    value: u32,
) -> Result<(), CaptureConfigValidationError> {
    if value != 0 {
        Ok(())
    } else {
        Err(CaptureConfigValidationError::GridDimension { field, value })
    }
}

fn validate_capture_float(
    field: &'static str,
    value: f32,
    min: f32,
    max: f32,
) -> Result<(), CaptureConfigValidationError> {
    if value.is_finite() && (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(CaptureConfigValidationError::FloatRange {
            field,
            min,
            max,
            value,
        })
    }
}

fn validate_capture_source(
    platform: CapturePlatform,
    source: &str,
    enabled: bool,
) -> Result<(), CaptureConfigValidationError> {
    let source = source.trim();
    let platform_name = match platform {
        CapturePlatform::WindowsDesktopDuplication => "Windows Desktop Duplication",
        CapturePlatform::LinuxPipeWire => "Linux PipeWire",
        CapturePlatform::MacosScreenCaptureKit => "macOS ScreenCaptureKit",
        CapturePlatform::Unsupported => "this platform",
    };
    if source.is_empty() {
        return Err(CaptureConfigValidationError::Source {
            platform: platform_name,
            reason: "the source must not be empty",
        });
    }
    if source.len() > 1024 {
        return Err(CaptureConfigValidationError::Source {
            platform: platform_name,
            reason: "the source exceeds 1024 bytes",
        });
    }
    if source.chars().any(char::is_control) {
        return Err(CaptureConfigValidationError::Source {
            platform: platform_name,
            reason: "the source contains control characters",
        });
    }
    if enabled
        && matches!(platform, CapturePlatform::LinuxPipeWire)
        && !source.eq_ignore_ascii_case("auto")
    {
        return Err(CaptureConfigValidationError::Source {
            platform: platform_name,
            reason: "portal-managed capture requires source = \"auto\"",
        });
    }
    if matches!(platform, CapturePlatform::MacosScreenCaptureKit)
        && !is_valid_macos_capture_source(source)
    {
        return Err(CaptureConfigValidationError::Source {
            platform: platform_name,
            reason: "expected auto, primary_display, session_scoped, or display:<canonical UUID>",
        });
    }
    Ok(())
}

fn is_valid_macos_capture_source(source: &str) -> bool {
    matches!(source, "auto" | "primary_display" | "session_scoped")
        || source.strip_prefix("display:").is_some_and(|value| {
            value.len() == 36
                && Uuid::parse_str(value)
                    .is_ok_and(|uuid| uuid.hyphenated().to_string().eq_ignore_ascii_case(value))
        })
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            enabled: defaults::capture_enabled(),
            source: defaults::capture_source(),
            capture_fps: defaults::capture_fps(),
            cadence: CaptureCadenceMode::default(),
            grid_cols: defaults::capture_grid_cols(),
            grid_rows: defaults::capture_grid_rows(),
            publication_memory_bytes: None,
            smoothing: defaults::capture_smoothing(),
            scene_cut_threshold: defaults::capture_scene_cut_threshold(),
            letterbox: false,
            letterbox_threshold: defaults::capture_letterbox_threshold(),
            saturation: defaults::unit_scale(),
            brightness: defaults::unit_scale(),
            gamma: defaults::unit_scale(),
            target_led_white_x: defaults::capture_target_led_white_x(),
            target_led_white_y: defaults::capture_target_led_white_y(),
            target_led_reference_white_nits: defaults::capture_target_led_reference_white_nits(),
            target_led_peak_nits: defaults::capture_target_led_peak_nits(),
            exposure_ev: defaults::capture_exposure_ev(),
            restore_token: None,
        }
    }
}
