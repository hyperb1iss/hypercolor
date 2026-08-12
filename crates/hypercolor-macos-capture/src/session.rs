use crate::MacosCaptureError;

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
        })
    }
}

impl Default for MacosStreamRequest {
    fn default() -> Self {
        Self {
            cadence: MacosCaptureCadence::FramesPerSecond(60),
            cursor_composed: true,
        }
    }
}
