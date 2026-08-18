//! Shared CPU and GPU contract for LED-targeted capture color processing.

use std::num::NonZeroU32;
use std::time::Duration;

use thiserror::Error;

use hypercolor_types::canvas::linear_to_srgb_u8;

use super::frame::{
    CaptureColorSpace, CaptureDynamicRange, CaptureLuminanceContext, CapturePositiveScalar,
    CaptureTransferFunction, KnownCaptureColorimetry,
};

/// Exact cache revision of the shared LED tone-mapping algorithm.
pub const LED_TONE_MAP_ALGORITHM_REVISION: NonZeroU32 = NonZeroU32::MIN;
/// Duration of an SDR/HDR curve transition.
pub const LED_TONE_MAP_TRANSITION_DURATION: Duration = Duration::from_millis(250);
/// Lowest accepted user exposure.
pub const LED_TONE_MAP_MIN_EXPOSURE_EV: f32 = -8.0;
/// Highest accepted user exposure.
pub const LED_TONE_MAP_MAX_EXPOSURE_EV: f32 = 8.0;

const D65_X: f32 = 0.3127;
const D65_Y: f32 = 0.3290;
const DEFAULT_REFERENCE_WHITE_NITS: f32 = 203.0;
const DEFAULT_PEAK_NITS: f32 = 406.0;
const MIN_REFERENCE_WHITE_NITS: f32 = 1.0;
const MAX_REFERENCE_WHITE_NITS: f32 = 5_000.0;
const MIN_PEAK_NITS: f32 = 1.0;
const MAX_PEAK_NITS: f32 = 10_000.0;

const SRGB_TO_XYZ: Matrix3 = Matrix3([
    [0.412_390_8, 0.357_584_33, 0.180_480_8],
    [0.212_639, 0.715_168_65, 0.072_192_32],
    [0.019_330_82, 0.119_194_78, 0.950_532_14],
]);
const DISPLAY_P3_TO_XYZ: Matrix3 = Matrix3([
    [0.486_570_95, 0.265_667_7, 0.198_217_29],
    [0.228_974_57, 0.691_738_55, 0.079_286_91],
    [0.0, 0.045_113_38, 1.043_944_4],
]);
const REC2020_TO_XYZ: Matrix3 = Matrix3([
    [0.636_958_06, 0.144_616_9, 0.168_880_98],
    [0.262_700_2, 0.677_998_07, 0.059_301_715],
    [0.0, 0.028_072_694, 1.060_985_1],
]);
const BRADFORD: Matrix3 = Matrix3([
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
]);
const BRADFORD_INVERSE: Matrix3 = Matrix3([
    [0.986_992_9, -0.147_054_3, 0.159_962_7],
    [0.432_305_3, 0.518_360_3, 0.049_291_2],
    [-0.008_528_7, 0.040_042_8, 0.968_486_7],
]);
const IDENTITY: Matrix3 = Matrix3([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct CanonicalScalar(u32);

impl CanonicalScalar {
    const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    fn try_new(value: f32) -> Result<Self, LedToneMapCalibrationError> {
        if !value.is_finite() {
            return Err(LedToneMapCalibrationError::NonFiniteScalar);
        }
        Ok(if value == 0.0 {
            Self::from_bits(0.0_f32.to_bits())
        } else {
            Self::from_bits(value.to_bits())
        })
    }

    const fn value(self) -> f32 {
        f32::from_bits(self.0)
    }
}

/// Validated target LED calibration and authoritative user exposure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LedToneMapCalibration {
    target_white_x: CanonicalScalar,
    target_white_y: CanonicalScalar,
    target_reference_white_nits: CanonicalScalar,
    target_peak_nits: CanonicalScalar,
    exposure_ev: CanonicalScalar,
}

impl LedToneMapCalibration {
    /// Nominal D65 calibration with one stop of HDR output headroom.
    pub const DEFAULT: Self = Self {
        target_white_x: CanonicalScalar::from_bits(D65_X.to_bits()),
        target_white_y: CanonicalScalar::from_bits(D65_Y.to_bits()),
        target_reference_white_nits: CanonicalScalar::from_bits(
            DEFAULT_REFERENCE_WHITE_NITS.to_bits(),
        ),
        target_peak_nits: CanonicalScalar::from_bits(DEFAULT_PEAK_NITS.to_bits()),
        exposure_ev: CanonicalScalar::from_bits(0.0_f32.to_bits()),
    };

    /// Validate one complete target calibration without clamping any field.
    ///
    /// # Errors
    ///
    /// Rejects non-finite values, chromaticities outside the CIE xy triangle,
    /// luminance outside the specified ranges, a non-increasing peak, and
    /// exposure outside `-8..=8` EV.
    pub fn try_new(
        target_white_x: f32,
        target_white_y: f32,
        target_reference_white_nits: f32,
        target_peak_nits: f32,
        exposure_ev: f32,
    ) -> Result<Self, LedToneMapCalibrationError> {
        let calibration = Self {
            target_white_x: CanonicalScalar::try_new(target_white_x)?,
            target_white_y: CanonicalScalar::try_new(target_white_y)?,
            target_reference_white_nits: CanonicalScalar::try_new(target_reference_white_nits)?,
            target_peak_nits: CanonicalScalar::try_new(target_peak_nits)?,
            exposure_ev: CanonicalScalar::try_new(exposure_ev)?,
        };
        calibration.validate()?;
        Ok(calibration)
    }

    fn validate(self) -> Result<(), LedToneMapCalibrationError> {
        let x = self.target_white_x();
        let y = self.target_white_y();
        if x <= 0.0 || y <= 0.0 || x + y >= 1.0 {
            return Err(LedToneMapCalibrationError::WhitePointOutsideChromaticityTriangle);
        }
        if !(MIN_REFERENCE_WHITE_NITS..=MAX_REFERENCE_WHITE_NITS)
            .contains(&self.target_reference_white_nits())
        {
            return Err(LedToneMapCalibrationError::ReferenceWhiteOutOfRange);
        }
        if !(MIN_PEAK_NITS..=MAX_PEAK_NITS).contains(&self.target_peak_nits()) {
            return Err(LedToneMapCalibrationError::PeakOutOfRange);
        }
        if self.target_peak_nits() <= self.target_reference_white_nits() {
            return Err(LedToneMapCalibrationError::PeakNotAboveReferenceWhite);
        }
        if !(LED_TONE_MAP_MIN_EXPOSURE_EV..=LED_TONE_MAP_MAX_EXPOSURE_EV)
            .contains(&self.exposure_ev())
        {
            return Err(LedToneMapCalibrationError::ExposureOutOfRange);
        }
        Ok(())
    }

    const fn has_nominal_d65_white(self) -> bool {
        self.target_white_x.0 == Self::DEFAULT.target_white_x.0
            && self.target_white_y.0 == Self::DEFAULT.target_white_y.0
    }

    /// Target LED white-point x chromaticity.
    #[must_use]
    pub const fn target_white_x(self) -> f32 {
        self.target_white_x.value()
    }

    /// Target LED white-point y chromaticity.
    #[must_use]
    pub const fn target_white_y(self) -> f32 {
        self.target_white_y.value()
    }

    /// Target reference white in nits.
    #[must_use]
    pub const fn target_reference_white_nits(self) -> f32 {
        self.target_reference_white_nits.value()
    }

    /// Calibrated target peak in nits.
    #[must_use]
    pub const fn target_peak_nits(self) -> f32 {
        self.target_peak_nits.value()
    }

    /// Authoritative user exposure in EV.
    #[must_use]
    pub const fn exposure_ev(self) -> f32 {
        self.exposure_ev.value()
    }

    /// Target luminance contract used by resolved publication metadata.
    #[must_use]
    pub fn target_luminance(self) -> CaptureLuminanceContext {
        let reference_white = CapturePositiveScalar::try_new(self.target_reference_white_nits())
            .expect("validated target reference white remains positive and finite");
        let peak = CapturePositiveScalar::try_new(self.target_peak_nits())
            .expect("validated target peak remains positive and finite");
        CaptureLuminanceContext::new(reference_white, peak)
            .expect("validated target peak remains above reference white")
    }
}

impl Default for LedToneMapCalibration {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Invalid target LED calibration.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum LedToneMapCalibrationError {
    /// Every calibration scalar must be finite.
    #[error("LED tone-map calibration values must be finite")]
    NonFiniteScalar,
    /// White-point coordinates must be strictly inside the CIE xy triangle.
    #[error("target LED white point must be strictly inside the CIE xy triangle")]
    WhitePointOutsideChromaticityTriangle,
    /// Target reference white must be within `1..=5000` nits.
    #[error("target LED reference white must be within 1..=5000 nits")]
    ReferenceWhiteOutOfRange,
    /// Target peak must be within `1..=10000` nits.
    #[error("target LED peak must be within 1..=10000 nits")]
    PeakOutOfRange,
    /// A tone-mapping target requires positive highlight headroom.
    #[error("target LED peak must be strictly above reference white")]
    PeakNotAboveReferenceWhite,
    /// User exposure must be within `-8..=8` EV.
    #[error("LED tone-map exposure must be within -8..=8 EV")]
    ExposureOutOfRange,
}

/// GPU-layout-compatible constants consumed by the CPU parity implementation.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LedToneMapConstants {
    /// Source-linear RGB to calibrated target-linear RGB matrix rows.
    pub source_to_target: [[f32; 4]; 3],
    /// Source linear luminance coefficients followed by exposure multiplier.
    pub source_luminance_and_exposure: [f32; 4],
    /// Target reference ratio, source headroom, source reference, target peak.
    pub curve: [f32; 4],
}

impl LedToneMapConstants {
    /// Interpolate only the old and new curve coordinates with smoothstep.
    #[must_use]
    pub fn transition_from(mut self, previous: Self, linear_progress: f32) -> Self {
        let progress = smoothstep(linear_progress.clamp(0.0, 1.0));
        self.curve[0] = lerp(previous.curve[0], self.curve[0], progress);
        self.curve[1] = lerp(previous.curve[1], self.curve[1], progress);
        self.curve[3] = lerp(previous.curve[3], self.curve[3], progress);
        self
    }
}

/// Fully prepared per-sample color and tone-mapping contract.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreparedLedToneMap {
    constants: LedToneMapConstants,
    source_transfer: CaptureTransferFunction,
    output_transfer: CaptureTransferFunction,
}

impl PreparedLedToneMap {
    /// Prepare exact CPU and GPU constants for one resolved color pipeline.
    ///
    /// # Errors
    ///
    /// Rejects unsupported transfer functions, contradictory range metadata,
    /// or an HDR source without absolute luminance and positive headroom.
    pub fn prepare(
        source: KnownCaptureColorimetry,
        output: KnownCaptureColorimetry,
        calibration: LedToneMapCalibration,
    ) -> Result<Self, PreparedLedToneMapError> {
        calibration.validate()?;
        validate_transfer(source.transfer_function(), source.dynamic_range(), true)?;
        validate_transfer(output.transfer_function(), output.dynamic_range(), false)?;

        let source_matrix = color_space_matrix(source.color_space())?;
        let output_matrix = color_space_matrix(output.color_space())?;
        let source_to_target = if source.color_space() == output.color_space()
            && calibration.has_nominal_d65_white()
        {
            IDENTITY
        } else {
            let device_matrix = chromatic_adaptation(
                D65_X,
                D65_Y,
                calibration.target_white_x(),
                calibration.target_white_y(),
            )
            .multiply(output_matrix);
            device_matrix
                .inverse()
                .ok_or(PreparedLedToneMapError::SingularWhitePointTransform)?
                .multiply(source_matrix)
        };
        let source_luminance = source_matrix.padded_rows()[1];
        let (target_reference_ratio, source_headroom, source_reference_nits) =
            if source.dynamic_range() == CaptureDynamicRange::High {
                let luminance = source
                    .luminance()
                    .ok_or(PreparedLedToneMapError::MissingSourceLuminance)?;
                let reference = luminance.reference_white_nits().value();
                let peak = luminance.peak_nits().value();
                if peak <= reference {
                    return Err(PreparedLedToneMapError::SourcePeakNotAboveReferenceWhite);
                }
                (
                    calibration.target_reference_white_nits() / calibration.target_peak_nits(),
                    peak / reference,
                    reference,
                )
            } else {
                (1.0, 1.0, 1.0)
            };

        Ok(Self {
            constants: LedToneMapConstants {
                source_to_target: source_to_target.padded_rows(),
                source_luminance_and_exposure: [
                    source_luminance[0],
                    source_luminance[1],
                    source_luminance[2],
                    2.0_f32.powf(calibration.exposure_ev()),
                ],
                curve: [
                    target_reference_ratio,
                    source_headroom,
                    source_reference_nits,
                    calibration.target_peak_nits(),
                ],
            },
            source_transfer: source.transfer_function(),
            output_transfer: output.transfer_function(),
        })
    }

    /// Shared constants suitable for direct upload to a GPU uniform buffer.
    #[must_use]
    pub const fn constants(self) -> LedToneMapConstants {
        self.constants
    }

    /// Replace the prepared curve with a smooth transition from an older curve.
    #[must_use]
    pub fn transition_from(mut self, previous: Self, linear_progress: f32) -> Self {
        self.constants = self
            .constants
            .transition_from(previous.constants, linear_progress);
        self
    }

    /// Decode one source sample and apply the complete linear-light contract.
    #[must_use]
    pub fn decode_and_map(self, encoded: [u8; 4]) -> [f64; 4] {
        self.decode_and_map_source(encoded.map(|channel| f32::from(channel) / 255.0))
    }

    /// Decode one full-precision source-domain sample and apply the linear contract.
    ///
    /// Values are not clamped before transfer decoding. Extended-linear sources
    /// therefore retain diffuse and specular values above one until tone mapping.
    #[must_use]
    pub fn decode_and_map_source(self, source: [f32; 4]) -> [f64; 4] {
        let mut rgb = [
            self.decode_source_channel(source[0]),
            self.decode_source_channel(source[1]),
            self.decode_source_channel(source[2]),
        ];
        if self.source_transfer == CaptureTransferFunction::Hlg {
            rgb = hlg_scene_to_reference_linear(
                rgb,
                &self.constants.source_luminance_and_exposure[..3],
                self.constants.curve[1],
                self.constants.curve[2],
            );
        }
        let mapped = self.map_linear(rgb);
        [
            f64::from(mapped[0]),
            f64::from(mapped[1]),
            f64::from(mapped[2]),
            f64::from(source[3]),
        ]
    }

    /// Apply white point, exposure, gamut compression, and the prepared curve.
    #[must_use]
    pub fn map_linear(self, source_rgb: [f32; 3]) -> [f32; 3] {
        let constants = self.constants;
        let exposure = constants.source_luminance_and_exposure[3];
        let exposed = source_rgb.map(|channel| channel * exposure);
        let source_luminance =
            dot3(&constants.source_luminance_and_exposure[..3], exposed).max(0.0);
        let mapped_luminance = map_luminance(
            source_luminance,
            constants.curve[0],
            constants.curve[1],
            constants.curve[3],
        );
        let mut target = multiply_padded_rows(constants.source_to_target, exposed);
        if source_luminance > f32::EPSILON {
            let luminance_scale = mapped_luminance / source_luminance;
            target = target.map(|channel| channel * luminance_scale);
        } else {
            target = [0.0; 3];
        }
        let minimum = target.into_iter().fold(f32::INFINITY, f32::min);
        let maximum = target.into_iter().fold(f32::NEG_INFINITY, f32::max);
        if maximum - minimum <= 1.0e-5 {
            target = [mapped_luminance; 3];
        }
        compress_gamut(target, mapped_luminance)
    }

    /// Encode one spatially accumulated target-linear sample.
    #[must_use]
    pub fn encode(self, linear: [f64; 4]) -> [u8; 4] {
        let encode = |value: f64| match self.output_transfer {
            CaptureTransferFunction::Srgb => linear_to_srgb_u8(value as f32),
            CaptureTransferFunction::Linear => encode_byte(value as f32),
            CaptureTransferFunction::Rec709 => encode_byte(linear_to_rec709(value as f32)),
            CaptureTransferFunction::Rec2020 => encode_byte(linear_to_rec2020(value as f32)),
            CaptureTransferFunction::Pq
            | CaptureTransferFunction::Hlg
            | CaptureTransferFunction::Unknown => {
                unreachable!("prepared target transfer remains SDR")
            }
        };
        [
            encode(linear[0]),
            encode(linear[1]),
            encode(linear[2]),
            encode_byte(linear[3] as f32),
        ]
    }

    fn decode_source_channel(self, encoded: f32) -> f32 {
        match self.source_transfer {
            CaptureTransferFunction::Srgb => srgb_to_linear(encoded),
            CaptureTransferFunction::Linear => encoded,
            CaptureTransferFunction::Rec709 => rec709_to_linear(encoded),
            CaptureTransferFunction::Rec2020 => rec2020_to_linear(encoded),
            CaptureTransferFunction::Pq => pq_to_nits(encoded) / self.constants.curve[2],
            CaptureTransferFunction::Hlg => hlg_inverse_oetf(encoded),
            CaptureTransferFunction::Unknown => {
                unreachable!("prepared source transfer remains executable")
            }
        }
    }
}

/// Stateful frame-boundary transition between prepared SDR and HDR curves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LedToneMapCurveTransition {
    from: PreparedLedToneMap,
    target: PreparedLedToneMap,
    current: PreparedLedToneMap,
    started_at: Duration,
    active: bool,
}

impl LedToneMapCurveTransition {
    /// Start with one fully active curve and no transition marker.
    #[must_use]
    pub const fn new(initial: PreparedLedToneMap) -> Self {
        Self {
            from: initial,
            target: initial,
            current: initial,
            started_at: Duration::ZERO,
            active: false,
        }
    }

    /// Begin a full 250 ms transition at one frame boundary.
    ///
    /// Retargeting an active transition begins from its current interpolated
    /// curve and restarts the duration at the supplied frame timestamp.
    pub fn transition_to(&mut self, target: PreparedLedToneMap, frame_timestamp: Duration) {
        self.update(frame_timestamp);
        if self.current == target {
            self.from = target;
            self.target = target;
            self.active = false;
            return;
        }
        self.from = self.current;
        self.target = target;
        self.started_at = frame_timestamp;
        self.active = true;
    }

    /// Resolve the curve and exact transition marker for one frame boundary.
    #[must_use]
    pub fn sample(&mut self, frame_timestamp: Duration) -> LedToneMapTransitionSample {
        self.update(frame_timestamp);
        LedToneMapTransitionSample {
            prepared: self.current,
            suppress_scene_cut_bypass: self.active,
        }
    }

    #[cfg(all(test, feature = "macos-capture-fixtures"))]
    pub(super) const fn is_active(&self) -> bool {
        self.active
    }

    fn update(&mut self, frame_timestamp: Duration) {
        if !self.active {
            return;
        }
        let elapsed = frame_timestamp.saturating_sub(self.started_at);
        if elapsed >= LED_TONE_MAP_TRANSITION_DURATION {
            self.current = self.target;
            self.active = false;
            return;
        }
        let progress = elapsed.as_secs_f32() / LED_TONE_MAP_TRANSITION_DURATION.as_secs_f32();
        self.current = self.target.transition_from(self.from, progress);
    }
}

/// Prepared per-frame curve and its private smoothing-bypass suppression flag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LedToneMapTransitionSample {
    prepared: PreparedLedToneMap,
    suppress_scene_cut_bypass: bool,
}

impl LedToneMapTransitionSample {
    /// Interpolated curve for this frame.
    #[must_use]
    pub const fn prepared(self) -> PreparedLedToneMap {
        self.prepared
    }

    /// Whether temporal smoothers must suppress their scene-cut bypass.
    #[must_use]
    pub const fn suppress_scene_cut_bypass(self) -> bool {
        self.suppress_scene_cut_bypass
    }
}

/// Failure to prepare an executable tone-mapping contract.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum PreparedLedToneMapError {
    /// The calibration failed its public validation contract.
    #[error(transparent)]
    InvalidCalibration(#[from] LedToneMapCalibrationError),
    /// One required source or target color space is unknown.
    #[error("LED tone mapping requires known source and target primaries")]
    UnknownColorSpace,
    /// One source transfer function is outside the executable CPU/GPU contract.
    #[error("unsupported source transfer function for LED tone mapping: {0:?}")]
    UnsupportedSourceTransfer(CaptureTransferFunction),
    /// Output must use an SDR transfer function.
    #[error("unsupported output transfer function for LED tone mapping: {0:?}")]
    UnsupportedOutputTransfer(CaptureTransferFunction),
    /// HDR source metadata omitted absolute luminance.
    #[error("HDR LED tone mapping requires source luminance metadata")]
    MissingSourceLuminance,
    /// HDR source metadata must provide positive highlight headroom.
    #[error("HDR source peak must be strictly above source reference white")]
    SourcePeakNotAboveReferenceWhite,
    /// The calibrated target basis could not be inverted.
    #[error("target LED white-point transform is singular")]
    SingularWhitePointTransform,
}

fn validate_transfer(
    transfer: CaptureTransferFunction,
    dynamic_range: CaptureDynamicRange,
    source: bool,
) -> Result<(), PreparedLedToneMapError> {
    let valid = if source {
        matches!(
            (transfer, dynamic_range),
            (
                CaptureTransferFunction::Srgb | CaptureTransferFunction::Linear,
                CaptureDynamicRange::Standard
            ) | (
                CaptureTransferFunction::Rec709 | CaptureTransferFunction::Rec2020,
                CaptureDynamicRange::Standard
            ) | (
                CaptureTransferFunction::Pq
                    | CaptureTransferFunction::Hlg
                    | CaptureTransferFunction::Linear,
                CaptureDynamicRange::High
            )
        )
    } else {
        matches!(
            (transfer, dynamic_range),
            (
                CaptureTransferFunction::Srgb | CaptureTransferFunction::Linear,
                CaptureDynamicRange::Standard
            ) | (
                CaptureTransferFunction::Rec709 | CaptureTransferFunction::Rec2020,
                CaptureDynamicRange::Standard
            )
        )
    };
    if valid {
        return Ok(());
    }
    Err(if source {
        PreparedLedToneMapError::UnsupportedSourceTransfer(transfer)
    } else {
        PreparedLedToneMapError::UnsupportedOutputTransfer(transfer)
    })
}

fn color_space_matrix(color_space: CaptureColorSpace) -> Result<Matrix3, PreparedLedToneMapError> {
    match color_space {
        CaptureColorSpace::Srgb => Ok(SRGB_TO_XYZ),
        CaptureColorSpace::DisplayP3 => Ok(DISPLAY_P3_TO_XYZ),
        CaptureColorSpace::Rec2020 => Ok(REC2020_TO_XYZ),
        CaptureColorSpace::Unknown => Err(PreparedLedToneMapError::UnknownColorSpace),
    }
}

fn map_luminance(
    value: f32,
    reference_ratio: f32,
    source_headroom: f32,
    target_peak_nits: f32,
) -> f32 {
    if source_headroom <= 1.0 {
        return value.min(1.0) * reference_ratio;
    }
    let target_reference_nits = reference_ratio * target_peak_nits;
    let source_peak_nits = target_reference_nits * source_headroom;
    if source_peak_nits <= target_peak_nits {
        return (value * reference_ratio).clamp(0.0, 1.0);
    }

    let source_peak_pq = nits_to_pq(source_peak_nits);
    let maximum_luminance = nits_to_pq(target_peak_nits) / source_peak_pq;
    let input_pq = nits_to_pq(value * target_reference_nits) / source_peak_pq;
    let knee_start = 1.5 * maximum_luminance - 0.5;
    if input_pq < knee_start {
        return (value * reference_ratio).clamp(0.0, 1.0);
    }
    let t = ((input_pq - knee_start) / (1.0 - knee_start)).clamp(0.0, 1.0);
    let t_squared = t * t;
    let t_cubed = t_squared * t;
    let output_pq = (2.0 * t_cubed - 3.0 * t_squared + 1.0) * knee_start
        + (t_cubed - 2.0 * t_squared + t) * (1.0 - knee_start)
        + (-2.0 * t_cubed + 3.0 * t_squared) * maximum_luminance;
    (pq_to_nits(output_pq * source_peak_pq) / target_peak_nits).clamp(0.0, 1.0)
}

fn compress_gamut(rgb: [f32; 3], luminance: f32) -> [f32; 3] {
    let neutral = luminance.clamp(0.0, 1.0);
    let mut scale = 1.0_f32;
    for channel in rgb {
        let chroma = channel - neutral;
        if channel < 0.0 {
            scale = scale.min(neutral / -chroma);
        } else if channel > 1.0 {
            scale = scale.min((1.0 - neutral) / chroma);
        }
    }
    rgb.map(|channel| (neutral + (channel - neutral) * scale).clamp(0.0, 1.0))
}

fn chromatic_adaptation(source_x: f32, source_y: f32, target_x: f32, target_y: f32) -> Matrix3 {
    let source_white = xyz_from_xy(source_x, source_y);
    let target_white = xyz_from_xy(target_x, target_y);
    let source_cone = BRADFORD.multiply_vector(source_white);
    let target_cone = BRADFORD.multiply_vector(target_white);
    let scale = Matrix3([
        [target_cone[0] / source_cone[0], 0.0, 0.0],
        [0.0, target_cone[1] / source_cone[1], 0.0],
        [0.0, 0.0, target_cone[2] / source_cone[2]],
    ]);
    BRADFORD_INVERSE.multiply(scale).multiply(BRADFORD)
}

fn xyz_from_xy(x: f32, y: f32) -> [f64; 3] {
    let x = f64::from(x);
    let y = f64::from(y);
    [x / y, 1.0, (1.0 - x - y) / y]
}

fn pq_to_nits(encoded: f32) -> f32 {
    const M1: f32 = 2_610.0 / 16_384.0;
    const M2: f32 = 2_523.0 / 32.0;
    const C1: f32 = 3_424.0 / 4_096.0;
    const C2: f32 = 2_413.0 / 128.0;
    const C3: f32 = 2_392.0 / 128.0;

    let power = encoded.clamp(0.0, 1.0).powf(1.0 / M2);
    let numerator = (power - C1).max(0.0);
    let denominator = C2 - C3 * power;
    10_000.0 * (numerator / denominator).powf(1.0 / M1)
}

fn srgb_to_linear(encoded: f32) -> f32 {
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

fn rec709_to_linear(encoded: f32) -> f32 {
    if encoded < 0.081 {
        encoded / 4.5
    } else {
        ((encoded + 0.099) / 1.099).powf(1.0 / 0.45)
    }
}

fn linear_to_rec709(linear: f32) -> f32 {
    if linear < 0.018 {
        4.5 * linear
    } else {
        1.099 * linear.powf(0.45) - 0.099
    }
}

fn rec2020_to_linear(encoded: f32) -> f32 {
    const ALPHA: f32 = 1.099_296_8;
    const BETA: f32 = 0.018_053_97;
    if encoded < 4.5 * BETA {
        encoded / 4.5
    } else {
        ((encoded + ALPHA - 1.0) / ALPHA).powf(1.0 / 0.45)
    }
}

fn linear_to_rec2020(linear: f32) -> f32 {
    const ALPHA: f32 = 1.099_296_8;
    const BETA: f32 = 0.018_053_97;
    if linear < BETA {
        4.5 * linear
    } else {
        ALPHA * linear.powf(0.45) - (ALPHA - 1.0)
    }
}

fn hlg_inverse_oetf(encoded: f32) -> f32 {
    const A: f32 = 0.178_832_77;
    const B: f32 = 0.284_668_92;
    const C: f32 = 0.559_910_7;
    let encoded = encoded.max(0.0);
    if encoded <= 0.5 {
        encoded * encoded / 3.0
    } else {
        (((encoded - C) / A).exp() + B) / 12.0
    }
}

fn hlg_scene_to_reference_linear(
    scene_rgb: [f32; 3],
    source_luminance: &[f32],
    source_headroom: f32,
    source_reference_nits: f32,
) -> [f32; 3] {
    let source_peak_nits = source_reference_nits * source_headroom;
    let system_gamma = 1.2 + 0.42 * (source_peak_nits / 1_000.0).log10();
    let scene_luminance = dot3(source_luminance, scene_rgb).max(0.0);
    if scene_luminance <= f32::EPSILON {
        return [0.0; 3];
    }
    let ootf_scale = source_headroom * scene_luminance.powf(system_gamma - 1.0);
    scene_rgb.map(|channel| channel * ootf_scale)
}

fn nits_to_pq(nits: f32) -> f32 {
    const M1: f32 = 2_610.0 / 16_384.0;
    const M2: f32 = 2_523.0 / 32.0;
    const C1: f32 = 3_424.0 / 4_096.0;
    const C2: f32 = 2_413.0 / 128.0;
    const C3: f32 = 2_392.0 / 128.0;

    let power = (nits.max(0.0) / 10_000.0).powf(M1);
    ((C1 + C2 * power) / (1.0 + C3 * power)).powf(M2)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn encode_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn dot3(coefficients: &[f32], value: [f32; 3]) -> f32 {
    coefficients[0] * value[0] + coefficients[1] * value[1] + coefficients[2] * value[2]
}

fn multiply_padded_rows(rows: [[f32; 4]; 3], value: [f32; 3]) -> [f32; 3] {
    [
        dot3(&rows[0][..3], value),
        dot3(&rows[1][..3], value),
        dot3(&rows[2][..3], value),
    ]
}

fn smoothstep(value: f32) -> f32 {
    value * value * (3.0 - 2.0 * value)
}

fn lerp(from: f32, to: f32, progress: f32) -> f32 {
    from + (to - from) * progress
}

#[derive(Clone, Copy)]
struct Matrix3([[f64; 3]; 3]);

impl Matrix3 {
    fn multiply(self, right: Self) -> Self {
        let mut output = [[0.0; 3]; 3];
        for (row_index, row) in output.iter_mut().enumerate() {
            for (column_index, value) in row.iter_mut().enumerate() {
                *value = (0..3)
                    .map(|index| self.0[row_index][index] * right.0[index][column_index])
                    .sum();
            }
        }
        Self(output)
    }

    fn multiply_vector(self, value: [f64; 3]) -> [f64; 3] {
        self.0
            .map(|row| row[0] * value[0] + row[1] * value[1] + row[2] * value[2])
    }

    fn inverse(self) -> Option<Self> {
        let matrix = self.0;
        let determinant = matrix[0][0]
            * (matrix[1][1] * matrix[2][2] - matrix[1][2] * matrix[2][1])
            - matrix[0][1] * (matrix[1][0] * matrix[2][2] - matrix[1][2] * matrix[2][0])
            + matrix[0][2] * (matrix[1][0] * matrix[2][1] - matrix[1][1] * matrix[2][0]);
        if determinant.abs() <= f64::EPSILON {
            return None;
        }
        let inverse = 1.0 / determinant;
        Some(Self([
            [
                (matrix[1][1] * matrix[2][2] - matrix[1][2] * matrix[2][1]) * inverse,
                (matrix[0][2] * matrix[2][1] - matrix[0][1] * matrix[2][2]) * inverse,
                (matrix[0][1] * matrix[1][2] - matrix[0][2] * matrix[1][1]) * inverse,
            ],
            [
                (matrix[1][2] * matrix[2][0] - matrix[1][0] * matrix[2][2]) * inverse,
                (matrix[0][0] * matrix[2][2] - matrix[0][2] * matrix[2][0]) * inverse,
                (matrix[0][2] * matrix[1][0] - matrix[0][0] * matrix[1][2]) * inverse,
            ],
            [
                (matrix[1][0] * matrix[2][1] - matrix[1][1] * matrix[2][0]) * inverse,
                (matrix[0][1] * matrix[2][0] - matrix[0][0] * matrix[2][1]) * inverse,
                (matrix[0][0] * matrix[1][1] - matrix[0][1] * matrix[1][0]) * inverse,
            ],
        ]))
    }

    #[allow(clippy::cast_possible_truncation)]
    fn padded_rows(self) -> [[f32; 4]; 3] {
        self.0
            .map(|row| [row[0] as f32, row[1] as f32, row[2] as f32, 0.0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(reference: f32, peak: f32) -> CaptureLuminanceContext {
        CaptureLuminanceContext::new(
            CapturePositiveScalar::try_new(reference).expect("reference is valid"),
            CapturePositiveScalar::try_new(peak).expect("peak is valid"),
        )
        .expect("luminance is ordered")
    }

    fn hdr_source() -> KnownCaptureColorimetry {
        KnownCaptureColorimetry::try_new(
            CaptureColorSpace::Rec2020,
            CaptureTransferFunction::Pq,
            CaptureDynamicRange::High,
            Some(luminance(203.0, 1_000.0)),
        )
        .expect("HDR source is valid")
    }

    fn linear_hdr_source() -> KnownCaptureColorimetry {
        KnownCaptureColorimetry::try_new(
            CaptureColorSpace::Rec2020,
            CaptureTransferFunction::Linear,
            CaptureDynamicRange::High,
            Some(luminance(203.0, 1_000.0)),
        )
        .expect("extended-linear HDR source is valid")
    }

    fn hlg_hdr_source() -> KnownCaptureColorimetry {
        KnownCaptureColorimetry::try_new(
            CaptureColorSpace::Rec2020,
            CaptureTransferFunction::Hlg,
            CaptureDynamicRange::High,
            Some(luminance(203.0, 1_000.0)),
        )
        .expect("HLG source is valid")
    }

    #[test]
    fn golden_reference_white_and_highlight_shoulder() {
        let sdr = PreparedLedToneMap::prepare(
            KnownCaptureColorimetry::SRGB,
            KnownCaptureColorimetry::SRGB,
            LedToneMapCalibration::DEFAULT,
        )
        .expect("SDR curve prepares");
        assert_eq!(sdr.map_linear([1.0; 3]), [1.0; 3]);

        let hdr = PreparedLedToneMap::prepare(
            hdr_source(),
            KnownCaptureColorimetry::SRGB,
            LedToneMapCalibration::DEFAULT,
        )
        .expect("HDR curve prepares");
        assert_eq!(hdr.map_linear([1.0; 3]), [0.5; 3]);
        for (input_nits, expected) in [
            (300.0, 0.726_557_2),
            (406.0, 0.873_631_95),
            (600.0, 0.975_499_9),
            (1_000.0, 1.0),
        ] {
            let actual = hdr.map_linear([input_nits / 203.0; 3])[0];
            assert!((actual - expected).abs() < 2.0e-5, "{input_nits} nits");
        }
        let mut previous = 0.5;
        for index in 1..=64 {
            let input = 1.0 + (1_000.0 / 203.0 - 1.0) * index as f32 / 64.0;
            let output = hdr.map_linear([input; 3])[0];
            assert!(output >= previous);
            assert!(output <= 1.0);
            previous = output;
        }
        assert!((previous - 1.0).abs() < 1.0e-6);
    }

    #[test]
    fn extended_linear_hdr_uses_the_same_reference_white_curve() {
        let prepared = PreparedLedToneMap::prepare(
            linear_hdr_source(),
            KnownCaptureColorimetry::SRGB,
            LedToneMapCalibration::DEFAULT,
        )
        .expect("extended-linear HDR curve prepares");
        for (source_relative, expected) in [
            (1.0, 0.5),
            (300.0 / 203.0, 0.726_557_2),
            (406.0 / 203.0, 0.873_631_95),
            (600.0 / 203.0, 0.975_499_9),
            (1_000.0 / 203.0, 1.0),
        ] {
            let actual = prepared.map_linear([source_relative; 3]);
            assert!((actual[0] - expected).abs() < 2.0e-5);
            assert_eq!(actual[0], actual[1]);
            assert_eq!(actual[1], actual[2]);
        }
        assert_eq!(prepared.decode_and_map([255; 4]), [0.5, 0.5, 0.5, 1.0]);
        let specular = prepared.decode_and_map_source([1_000.0 / 203.0; 4]);
        assert!((specular[0] - 1.0).abs() < 1.0e-6);
        assert!(specular[3] > 4.9);
    }

    #[test]
    fn hlg_diffuse_white_and_specular_peak_share_the_hdr_curve() {
        let prepared = PreparedLedToneMap::prepare(
            hlg_hdr_source(),
            KnownCaptureColorimetry::SRGB,
            LedToneMapCalibration::DEFAULT,
        )
        .expect("HLG HDR curve prepares");
        let diffuse = prepared.decode_and_map_source([0.75, 0.75, 0.75, 1.0]);
        assert!((diffuse[0] - 0.5).abs() < 5.0e-4);
        assert_eq!(diffuse[0], diffuse[1]);
        assert_eq!(diffuse[1], diffuse[2]);
        let specular = prepared.decode_and_map_source([1.0; 4]);
        assert!((specular[0] - 1.0).abs() < 2.0e-5);
        assert_eq!(specular[0], specular[1]);
        assert_eq!(specular[1], specular[2]);
        let rec2020_luminance = &REC2020_TO_XYZ.padded_rows()[1][..3];
        let neutral_1k = hlg_scene_to_reference_linear(
            [1.0 / 12.0; 3],
            rec2020_luminance,
            1_000.0 / 203.0,
            203.0,
        );
        assert!((neutral_1k[0] - 0.249_739_05).abs() < 1.0e-6);
        let neutral_1600 = hlg_scene_to_reference_linear(
            [1.0 / 12.0; 3],
            rec2020_luminance,
            1_600.0 / 203.0,
            203.0,
        );
        assert!((neutral_1600[0] - 0.322_914_7).abs() < 1.0e-6);
        let chromatic = hlg_scene_to_reference_linear(
            [1.0, 1.0 / 12.0, 0.0],
            rec2020_luminance,
            1_000.0 / 203.0,
            203.0,
        );
        assert!((chromatic[0] - 3.920_275_2).abs() < 1.0e-6);
        assert!((chromatic[1] - 0.326_689_6).abs() < 1.0e-6);
        assert_eq!(chromatic[2], 0.0);
    }

    #[test]
    fn measured_white_and_wide_gamut_remain_finite_and_bounded() {
        let measured = LedToneMapCalibration::try_new(0.3457, 0.3585, 203.0, 406.0, 0.0)
            .expect("measured calibration is valid");
        let p3 = KnownCaptureColorimetry::try_new(
            CaptureColorSpace::DisplayP3,
            CaptureTransferFunction::Linear,
            CaptureDynamicRange::Standard,
            None,
        )
        .expect("P3 source is valid");
        let prepared = PreparedLedToneMap::prepare(p3, KnownCaptureColorimetry::SRGB, measured)
            .expect("measured curve prepares");
        let mapped = prepared.map_linear([1.0, 0.0, 1.0]);
        for (actual, expected) in mapped.into_iter().zip([0.811_240_8, 0.093_663_424, 1.0]) {
            assert!((actual - expected).abs() < 1.0e-6);
        }
        assert!(mapped.iter().all(|channel| channel.is_finite()));
        assert!(mapped.iter().all(|channel| (0.0..=1.0).contains(channel)));
        assert_ne!(
            prepared.constants().source_to_target,
            PreparedLedToneMap::prepare(
                p3,
                KnownCaptureColorimetry::SRGB,
                LedToneMapCalibration::DEFAULT,
            )
            .expect("D65 curve prepares")
            .constants()
            .source_to_target
        );
    }

    #[test]
    fn exposure_is_applied_in_linear_light() {
        let calibration = LedToneMapCalibration::try_new(D65_X, D65_Y, 203.0, 406.0, -1.0)
            .expect("negative exposure is valid");
        let prepared = PreparedLedToneMap::prepare(
            KnownCaptureColorimetry::SRGB,
            KnownCaptureColorimetry::SRGB,
            calibration,
        )
        .expect("exposed SDR curve prepares");
        assert_eq!(prepared.map_linear([1.0; 3]), [0.5; 3]);
        assert_eq!(std::mem::size_of::<LedToneMapConstants>(), 80);
        assert_eq!(std::mem::align_of::<LedToneMapConstants>(), 16);
    }

    #[test]
    fn transition_uses_the_contract_duration_and_monotonic_smoothstep() {
        assert_eq!(LED_TONE_MAP_TRANSITION_DURATION, Duration::from_millis(250));
        let sdr = PreparedLedToneMap::prepare(
            KnownCaptureColorimetry::SRGB,
            KnownCaptureColorimetry::SRGB,
            LedToneMapCalibration::DEFAULT,
        )
        .expect("SDR curve prepares");
        let hdr = PreparedLedToneMap::prepare(
            hdr_source(),
            KnownCaptureColorimetry::SRGB,
            LedToneMapCalibration::DEFAULT,
        )
        .expect("HDR curve prepares");
        assert_eq!(hdr.transition_from(sdr, 0.0).constants().curve[0], 1.0);
        assert_eq!(hdr.transition_from(sdr, 1.0).constants().curve[0], 0.5);
        assert_eq!(hdr.transition_from(sdr, 0.5).constants().curve[0], 0.75);

        let mut transition = LedToneMapCurveTransition::new(sdr);
        transition.transition_to(hdr, Duration::ZERO);
        let midpoint = transition.sample(Duration::from_millis(125));
        assert_eq!(midpoint.prepared().constants().curve[0], 0.75);
        assert!(midpoint.suppress_scene_cut_bypass());

        transition.transition_to(sdr, Duration::from_millis(125));
        let restarted_midpoint = transition.sample(Duration::from_millis(250));
        assert_eq!(restarted_midpoint.prepared().constants().curve[0], 0.875);
        assert!(restarted_midpoint.suppress_scene_cut_bypass());
        let completed = transition.sample(Duration::from_millis(375));
        assert_eq!(completed.prepared(), sdr);
        assert!(!completed.suppress_scene_cut_bypass());
    }
}
