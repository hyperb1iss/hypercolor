use std::sync::Arc;

use crate::{MacosCaptureDynamicRange, MacosCaptureError, MacosStreamPreset};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MacosCaptureSelector {
    Auto,
    PrimaryDisplay,
    Display { source_id: Arc<str> },
    SessionScoped,
}

impl MacosCaptureSelector {
    pub fn parse(source: &str) -> Result<Self, MacosCaptureError> {
        match source.trim() {
            "auto" => Ok(Self::Auto),
            "primary_display" => Ok(Self::PrimaryDisplay),
            "session_scoped" => Ok(Self::SessionScoped),
            source => {
                let Some(uuid) = source.strip_prefix("display:") else {
                    return Err(MacosCaptureError::InvalidSourceSelector(source.to_owned()));
                };
                let uuid = uuid::Uuid::parse_str(uuid)
                    .map_err(|_| MacosCaptureError::InvalidSourceSelector(source.to_owned()))?;
                Ok(Self::Display {
                    source_id: Arc::from(format!("display:{}", uuid.hyphenated())),
                })
            }
        }
    }

    #[must_use]
    pub fn configured_source(&self) -> &str {
        match self {
            Self::Auto => "auto",
            Self::PrimaryDisplay => "primary_display",
            Self::Display { source_id } => source_id,
            Self::SessionScoped => "session_scoped",
        }
    }

    #[must_use]
    pub fn matches_display(&self, source_id: &str, primary: bool) -> bool {
        match self {
            Self::Auto | Self::PrimaryDisplay => primary,
            Self::Display {
                source_id: configured,
            } => configured.as_ref() == source_id,
            Self::SessionScoped => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosCaptureContentStyle {
    Window,
    MultipleWindows,
    Application,
    MultipleApplications,
    Mixed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum MacosCaptureSelection {
    #[default]
    None,
    Display {
        source_id: Arc<str>,
    },
    SessionScoped {
        content_style: MacosCaptureContentStyle,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MacosCaptureCadence {
    NativeRefresh,
    FramesPerSecond(u32),
}

impl MacosCaptureCadence {
    pub(crate) fn timescale(self) -> Result<Option<i32>, MacosCaptureError> {
        match self {
            Self::NativeRefresh => Ok(None),
            Self::FramesPerSecond(0) => Err(MacosCaptureError::InvalidCadence(0)),
            Self::FramesPerSecond(value) => i32::try_from(value)
                .map(Some)
                .map_err(|_| MacosCaptureError::InvalidCadence(value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacosStreamRequest {
    pub cadence: MacosCaptureCadence,
    pub cursor_composed: bool,
    pub dynamic_range: MacosCaptureDynamicRange,
}

impl MacosStreamRequest {
    pub fn new(
        cadence: MacosCaptureCadence,
        cursor_composed: bool,
    ) -> Result<Self, MacosCaptureError> {
        cadence.timescale()?;
        Ok(Self {
            cadence,
            cursor_composed,
            dynamic_range: MacosCaptureDynamicRange::Sdr,
        })
    }

    pub fn new_hdr(
        cadence: MacosCaptureCadence,
        cursor_composed: bool,
    ) -> Result<Self, MacosCaptureError> {
        cadence.timescale()?;
        Ok(Self {
            cadence,
            cursor_composed,
            dynamic_range: MacosCaptureDynamicRange::Hdr,
        })
    }

    #[must_use]
    pub const fn preset(self) -> MacosStreamPreset {
        match self.dynamic_range {
            MacosCaptureDynamicRange::Sdr => MacosStreamPreset::SdrDefault,
            MacosCaptureDynamicRange::Hdr => MacosStreamPreset::CaptureHdrStreamCanonicalDisplay,
        }
    }
}

impl Default for MacosStreamRequest {
    fn default() -> Self {
        Self {
            cadence: MacosCaptureCadence::FramesPerSecond(60),
            cursor_composed: true,
            dynamic_range: MacosCaptureDynamicRange::Sdr,
        }
    }
}
