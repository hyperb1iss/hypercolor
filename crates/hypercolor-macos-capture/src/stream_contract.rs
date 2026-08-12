use thiserror::Error;

use crate::{
    MacosCaptureColorimetry, MacosCapturePixelFormat, MacosColorRange, MacosTransferFunction,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosCaptureDynamicRange {
    Sdr,
    Hdr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosStreamPreset {
    SdrDefault,
    CaptureHdrStreamCanonicalDisplay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosHostArchitecture {
    AppleSilicon,
    Intel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosRuntimeCapability {
    Present,
    Absent,
}

impl MacosRuntimeCapability {
    #[must_use]
    pub const fn is_present(self) -> bool {
        matches!(self, Self::Present)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacosTahoeRuntimeProbes {
    pub content_tone_mapping_info_symbol: MacosRuntimeCapability,
    pub screenshot_configuration_class: MacosRuntimeCapability,
    pub screenshot_dynamic_range_selector: MacosRuntimeCapability,
    pub screenshot_capture_selector: MacosRuntimeCapability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacosTahoeCapabilities {
    pub content_tone_mapping_info: MacosRuntimeCapability,
    pub dual_range_screenshots: MacosRuntimeCapability,
}

impl MacosTahoeCapabilities {
    #[must_use]
    pub const fn from_probes(probes: MacosTahoeRuntimeProbes) -> Self {
        let dual_range_screenshots = if probes.screenshot_configuration_class.is_present()
            && probes.screenshot_dynamic_range_selector.is_present()
            && probes.screenshot_capture_selector.is_present()
        {
            MacosRuntimeCapability::Present
        } else {
            MacosRuntimeCapability::Absent
        };
        Self {
            content_tone_mapping_info: probes.content_tone_mapping_info_symbol,
            dual_range_screenshots,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacosCaptureCapabilities {
    pub host_architecture: MacosHostArchitecture,
    pub translated_process: bool,
    pub hdr_stream: MacosRuntimeCapability,
    pub tahoe: MacosTahoeCapabilities,
}

impl MacosCaptureCapabilities {
    #[must_use]
    pub const fn from_runtime(
        host_architecture: MacosHostArchitecture,
        translated_process: bool,
        tahoe_probes: MacosTahoeRuntimeProbes,
    ) -> Self {
        let hdr_stream = match host_architecture {
            MacosHostArchitecture::AppleSilicon => MacosRuntimeCapability::Present,
            MacosHostArchitecture::Intel => MacosRuntimeCapability::Absent,
        };
        Self {
            host_architecture,
            translated_process,
            hdr_stream,
            tahoe: MacosTahoeCapabilities::from_probes(tahoe_probes),
        }
    }

    pub fn validate_dynamic_range(
        self,
        dynamic_range: MacosCaptureDynamicRange,
    ) -> Result<(), MacosStreamDeliveryRejection> {
        if dynamic_range == MacosCaptureDynamicRange::Hdr && !self.hdr_stream.is_present() {
            return Err(MacosStreamDeliveryRejection::UnsupportedIntelHdr);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacosConfiguredStream {
    pub requested_dynamic_range: MacosCaptureDynamicRange,
    pub requested_preset: MacosStreamPreset,
    pub configured_dynamic_range: MacosCaptureDynamicRange,
    pub configured_pixel_format: MacosCapturePixelFormat,
    pub configured_color_range: MacosColorRange,
}

impl MacosConfiguredStream {
    pub fn validate(self) -> Result<(), MacosStreamDeliveryRejection> {
        let expected_preset = match self.requested_dynamic_range {
            MacosCaptureDynamicRange::Sdr => MacosStreamPreset::SdrDefault,
            MacosCaptureDynamicRange::Hdr => MacosStreamPreset::CaptureHdrStreamCanonicalDisplay,
        };
        if self.requested_preset != expected_preset {
            return Err(MacosStreamDeliveryRejection::PresetMismatch {
                requested: self.requested_dynamic_range,
                preset: self.requested_preset,
            });
        }
        if self.configured_dynamic_range != self.requested_dynamic_range {
            return Err(
                MacosStreamDeliveryRejection::ConfiguredDynamicRangeMismatch {
                    requested: self.requested_dynamic_range,
                    configured: self.configured_dynamic_range,
                },
            );
        }
        validate_format_for_dynamic_range(
            self.configured_pixel_format,
            self.configured_dynamic_range,
        )?;
        validate_color_range(self.configured_pixel_format, self.configured_color_range)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MacosDeliveredFrameMetadata {
    pub dynamic_range: MacosCaptureDynamicRange,
    pub pixel_format: MacosCapturePixelFormat,
    pub color: MacosCaptureColorimetry,
    pub source_reference_white_nits: Option<f32>,
    pub content_headroom: Option<f32>,
}

impl MacosDeliveredFrameMetadata {
    pub fn new(
        pixel_format: MacosCapturePixelFormat,
        color: MacosCaptureColorimetry,
        source_reference_white_nits: Option<f32>,
        content_headroom: Option<f32>,
    ) -> Result<Self, MacosStreamDeliveryRejection> {
        color.validate_for(pixel_format).map_err(|_| {
            MacosStreamDeliveryRejection::MissingOrInvalidDeliveryMetadata("colorimetry")
        })?;
        validate_optional_positive(source_reference_white_nits, "source_reference_white_nits")?;
        if content_headroom.is_some_and(|headroom| !headroom.is_finite() || headroom < 1.0) {
            return Err(
                MacosStreamDeliveryRejection::MissingOrInvalidDeliveryMetadata("content_headroom"),
            );
        }
        let dynamic_range = delivered_dynamic_range(pixel_format, color.transfer)?;
        Ok(Self {
            dynamic_range,
            pixel_format,
            color,
            source_reference_white_nits,
            content_headroom,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MacosValidatedStreamDelivery {
    pub configured: MacosConfiguredStream,
    pub delivered: MacosDeliveredFrameMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MacosStreamDeliveryState {
    AwaitingFirstCompleteFrame(MacosConfiguredStream),
    Confirmed(MacosValidatedStreamDelivery),
    Rejected(MacosStreamDeliveryRejection),
}

#[derive(Debug, Clone)]
pub struct MacosStreamDeliveryValidator {
    state: MacosStreamDeliveryState,
}

impl MacosStreamDeliveryValidator {
    #[must_use]
    pub const fn new(configured: MacosConfiguredStream) -> Self {
        Self {
            state: MacosStreamDeliveryState::AwaitingFirstCompleteFrame(configured),
        }
    }

    #[must_use]
    pub const fn state(&self) -> &MacosStreamDeliveryState {
        &self.state
    }

    pub fn validate_configuration(&mut self) -> Result<(), MacosStreamDeliveryRejection> {
        let MacosStreamDeliveryState::AwaitingFirstCompleteFrame(configured) = self.state else {
            return self.current_result().map(|_| ());
        };
        configured
            .validate()
            .map_err(|rejection| self.reject(rejection))
    }

    pub fn observe_first_complete(
        &mut self,
        delivered: Option<MacosDeliveredFrameMetadata>,
    ) -> Result<MacosValidatedStreamDelivery, MacosStreamDeliveryRejection> {
        let MacosStreamDeliveryState::AwaitingFirstCompleteFrame(configured) = self.state else {
            return self.current_result();
        };
        configured
            .validate()
            .map_err(|rejection| self.reject(rejection))?;
        let delivered = delivered
            .ok_or_else(|| self.reject(MacosStreamDeliveryRejection::MissingFirstCompleteFrame))?;
        if delivered.dynamic_range != configured.configured_dynamic_range {
            return Err(self.reject(
                MacosStreamDeliveryRejection::DeliveredDynamicRangeMismatch {
                    configured: configured.configured_dynamic_range,
                    delivered: delivered.dynamic_range,
                },
            ));
        }
        if delivered.pixel_format != configured.configured_pixel_format {
            return Err(
                self.reject(MacosStreamDeliveryRejection::DeliveredPixelFormatMismatch {
                    configured: configured.configured_pixel_format,
                    delivered: delivered.pixel_format,
                }),
            );
        }
        if delivered.color.range != configured.configured_color_range {
            return Err(
                self.reject(MacosStreamDeliveryRejection::DeliveredColorRangeMismatch {
                    configured: configured.configured_color_range,
                    delivered: delivered.color.range,
                }),
            );
        }
        validate_format_for_dynamic_range(delivered.pixel_format, delivered.dynamic_range)
            .map_err(|rejection| self.reject(rejection))?;
        let validated = MacosValidatedStreamDelivery {
            configured,
            delivered,
        };
        self.state = MacosStreamDeliveryState::Confirmed(validated);
        Ok(validated)
    }

    pub fn finish_without_complete_frame(
        &mut self,
    ) -> Result<MacosValidatedStreamDelivery, MacosStreamDeliveryRejection> {
        match self.state {
            MacosStreamDeliveryState::AwaitingFirstCompleteFrame(_) => {
                Err(self.reject(MacosStreamDeliveryRejection::MissingFirstCompleteFrame))
            }
            _ => self.current_result(),
        }
    }

    pub fn reject_delivery(
        &mut self,
        rejection: MacosStreamDeliveryRejection,
    ) -> MacosStreamDeliveryRejection {
        self.reject(rejection)
    }

    fn current_result(&self) -> Result<MacosValidatedStreamDelivery, MacosStreamDeliveryRejection> {
        match self.state {
            MacosStreamDeliveryState::Confirmed(delivery) => Ok(delivery),
            MacosStreamDeliveryState::Rejected(rejection) => Err(rejection),
            MacosStreamDeliveryState::AwaitingFirstCompleteFrame(_) => {
                Err(MacosStreamDeliveryRejection::MissingFirstCompleteFrame)
            }
        }
    }

    fn reject(&mut self, rejection: MacosStreamDeliveryRejection) -> MacosStreamDeliveryRejection {
        self.state = MacosStreamDeliveryState::Rejected(rejection);
        rejection
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MacosStreamDeliveryRejection {
    #[error("ScreenCaptureKit HDR capture is unsupported on Intel hosts")]
    UnsupportedIntelHdr,
    #[error("{preset:?} does not request {requested:?} capture")]
    PresetMismatch {
        requested: MacosCaptureDynamicRange,
        preset: MacosStreamPreset,
    },
    #[error("requested {requested:?} capture but configuration resolved {configured:?}")]
    ConfiguredDynamicRangeMismatch {
        requested: MacosCaptureDynamicRange,
        configured: MacosCaptureDynamicRange,
    },
    #[error("configured {configured:?} capture but first frame delivered {delivered:?}")]
    DeliveredDynamicRangeMismatch {
        configured: MacosCaptureDynamicRange,
        delivered: MacosCaptureDynamicRange,
    },
    #[error("configured {configured:?} pixels but first frame delivered {delivered:?}")]
    DeliveredPixelFormatMismatch {
        configured: MacosCapturePixelFormat,
        delivered: MacosCapturePixelFormat,
    },
    #[error("configured {configured:?} range but first frame delivered {delivered:?}")]
    DeliveredColorRangeMismatch {
        configured: MacosColorRange,
        delivered: MacosColorRange,
    },
    #[error("{format:?} is not a supported {dynamic_range:?} stream format")]
    UnsupportedFormatForDynamicRange {
        format: MacosCapturePixelFormat,
        dynamic_range: MacosCaptureDynamicRange,
    },
    #[error("first complete frame was not delivered")]
    MissingFirstCompleteFrame,
    #[error("first complete frame lacks valid {0}")]
    MissingOrInvalidDeliveryMetadata(&'static str),
}

fn validate_optional_positive(
    value: Option<f32>,
    field: &'static str,
) -> Result<(), MacosStreamDeliveryRejection> {
    if value.is_some_and(|value| !value.is_finite() || value <= 0.0) {
        return Err(MacosStreamDeliveryRejection::MissingOrInvalidDeliveryMetadata(field));
    }
    Ok(())
}

fn delivered_dynamic_range(
    format: MacosCapturePixelFormat,
    transfer: MacosTransferFunction,
) -> Result<MacosCaptureDynamicRange, MacosStreamDeliveryRejection> {
    match format {
        MacosCapturePixelFormat::Bgra8 => match transfer {
            MacosTransferFunction::Pq | MacosTransferFunction::Hlg => {
                Err(MacosStreamDeliveryRejection::MissingOrInvalidDeliveryMetadata("dynamic_range"))
            }
            _ => Ok(MacosCaptureDynamicRange::Sdr),
        },
        MacosCapturePixelFormat::Argb2101010 | MacosCapturePixelFormat::Rgba16Float => {
            Ok(MacosCaptureDynamicRange::Hdr)
        }
        MacosCapturePixelFormat::Yuv420VideoRange
        | MacosCapturePixelFormat::Yuv420FullRange
        | MacosCapturePixelFormat::Yuv44410BiPlanar => match transfer {
            MacosTransferFunction::Pq | MacosTransferFunction::Hlg => {
                Ok(MacosCaptureDynamicRange::Hdr)
            }
            _ => Ok(MacosCaptureDynamicRange::Sdr),
        },
    }
}

fn validate_format_for_dynamic_range(
    format: MacosCapturePixelFormat,
    dynamic_range: MacosCaptureDynamicRange,
) -> Result<(), MacosStreamDeliveryRejection> {
    let supported = match dynamic_range {
        MacosCaptureDynamicRange::Sdr => format == MacosCapturePixelFormat::Bgra8,
        MacosCaptureDynamicRange::Hdr => matches!(
            format,
            MacosCapturePixelFormat::Argb2101010
                | MacosCapturePixelFormat::Rgba16Float
                | MacosCapturePixelFormat::Yuv420VideoRange
                | MacosCapturePixelFormat::Yuv420FullRange
                | MacosCapturePixelFormat::Yuv44410BiPlanar
        ),
    };
    if supported {
        Ok(())
    } else {
        Err(
            MacosStreamDeliveryRejection::UnsupportedFormatForDynamicRange {
                format,
                dynamic_range,
            },
        )
    }
}

fn validate_color_range(
    format: MacosCapturePixelFormat,
    range: MacosColorRange,
) -> Result<(), MacosStreamDeliveryRejection> {
    let valid = match format {
        MacosCapturePixelFormat::Yuv420VideoRange => range == MacosColorRange::Video,
        MacosCapturePixelFormat::Bgra8
        | MacosCapturePixelFormat::Argb2101010
        | MacosCapturePixelFormat::Rgba16Float
        | MacosCapturePixelFormat::Yuv420FullRange => range == MacosColorRange::Full,
        MacosCapturePixelFormat::Yuv44410BiPlanar => true,
    };
    if valid {
        Ok(())
    } else {
        Err(
            MacosStreamDeliveryRejection::MissingOrInvalidDeliveryMetadata(
                "configured_color_range",
            ),
        )
    }
}
