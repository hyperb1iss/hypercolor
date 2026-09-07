//! Off-platform stand-ins for the ScreenCaptureKit session family.
//!
//! Neutral core compiles the macOS capture backend on every target so one
//! shared adapter can be verified anywhere. These types carry the native
//! session's public surface without any native ownership: constructors
//! report the platform as unsupported and instance methods are unreachable
//! because no value can exist.

use std::sync::Arc;

use crate::{
    MacosCaptureCallbackDiagnostics, MacosCaptureCapabilities, MacosCaptureError,
    MacosCaptureSelection, MacosCaptureSelector, MacosFrameMailbox, MacosProtectedSourceState,
    MacosStreamRequest, MacosTahoeSelectionCapabilities,
};

const UNSUPPORTED: MacosCaptureError =
    MacosCaptureError::CapabilityProbeFailed("ScreenCaptureKit is unavailable on this platform");

/// Stub ScreenCaptureKit session on hosts without ScreenCaptureKit.
#[derive(Debug)]
pub enum MacosScreenCaptureSession {}

/// Stub stream-request transaction on hosts without ScreenCaptureKit.
#[derive(Debug)]
pub enum MacosStreamRequestTransaction {}

/// Stub screenshot reference capture on hosts without ScreenCaptureKit.
#[derive(Debug, Clone)]
pub enum MacosScreenshotReferenceCapture {}

impl MacosScreenCaptureSession {
    /// Reports that the native capability probe is unavailable on this platform.
    ///
    /// # Errors
    ///
    /// Always returns the unsupported-platform capability error.
    pub fn capabilities() -> Result<MacosCaptureCapabilities, MacosCaptureError> {
        Err(UNSUPPORTED)
    }

    /// Reports that native sessions cannot be constructed on this platform.
    ///
    /// # Errors
    ///
    /// Always returns the unsupported-platform capability error.
    pub fn new(
        _request: MacosStreamRequest,
        _selector: MacosCaptureSelector,
    ) -> Result<Self, MacosCaptureError> {
        Err(UNSUPPORTED)
    }

    /// Reports that native sessions cannot be constructed on this platform.
    ///
    /// # Errors
    ///
    /// Always returns the unsupported-platform capability error.
    pub fn new_with_pool_admission<F, A>(
        _request: MacosStreamRequest,
        _selector: MacosCaptureSelector,
        _reserve_pool: F,
    ) -> Result<Self, MacosCaptureError>
    where
        F: Fn(u64, u64) -> Result<A, MacosCaptureError> + Send + Sync + 'static,
        A: Fn(u32, u64) -> Result<Arc<dyn Send + Sync>, MacosCaptureError> + Send + Sync + 'static,
    {
        Err(UNSUPPORTED)
    }

    /// Screen recording can never be authorized without ScreenCaptureKit.
    #[must_use]
    pub const fn screen_authorized() -> bool {
        false
    }

    /// Unreachable: no stub session can exist.
    #[must_use]
    pub fn request_authorization(&self) -> MacosProtectedSourceState {
        match *self {}
    }

    /// Unreachable: no stub session can exist.
    ///
    /// # Errors
    ///
    /// Never returns because no stub session can exist.
    pub fn present_picker(&self) -> Result<(), MacosCaptureError> {
        match *self {}
    }

    /// Unreachable: no stub session can exist.
    #[must_use]
    pub fn status(&self) -> MacosProtectedSourceState {
        match *self {}
    }

    /// Unreachable: no stub session can exist.
    #[must_use]
    pub fn selection(&self) -> MacosCaptureSelection {
        match *self {}
    }

    /// Unreachable: no stub session can exist.
    #[must_use]
    pub fn selection_revision(&self) -> u64 {
        match *self {}
    }

    /// Unreachable: no stub session can exist.
    #[must_use]
    pub fn tahoe_selection_capabilities(&self) -> Option<MacosTahoeSelectionCapabilities> {
        match *self {}
    }

    /// Unreachable: no stub session can exist.
    ///
    /// # Errors
    ///
    /// Never returns because no stub session can exist.
    pub fn capture_screenshot_reference_with_identity<F>(
        &self,
        _completion: F,
    ) -> Result<(), MacosCaptureError>
    where
        F: FnOnce(Result<MacosScreenshotReferenceCapture, MacosCaptureError>) + Send + 'static,
    {
        match *self {}
    }

    /// Unreachable: no stub session can exist.
    #[must_use]
    pub fn mailbox(&self) -> MacosFrameMailbox {
        match *self {}
    }

    /// Unreachable: no stub session can exist.
    #[must_use]
    pub fn diagnostics(&self) -> MacosCaptureCallbackDiagnostics {
        match *self {}
    }

    /// Unreachable: no stub session can exist.
    pub fn stop(&self) {
        match *self {}
    }

    /// Unreachable: no stub session can exist.
    pub fn set_capture_active(&self, _active: bool) {
        match *self {}
    }

    /// Unreachable: no stub session can exist.
    ///
    /// # Errors
    ///
    /// Never returns because no stub session can exist.
    pub fn begin_stream_request(
        &self,
        _request: MacosStreamRequest,
    ) -> Result<MacosStreamRequestTransaction, MacosCaptureError> {
        match *self {}
    }
}

impl MacosStreamRequestTransaction {
    /// Unreachable: no stub transaction can exist.
    #[must_use]
    pub fn generation(&self) -> u64 {
        match *self {}
    }

    /// Unreachable: no stub transaction can exist.
    ///
    /// # Errors
    ///
    /// Never returns because no stub transaction can exist.
    pub fn wait(self) -> Result<(), MacosCaptureError> {
        match self {}
    }
}

impl MacosScreenshotReferenceCapture {
    /// Unreachable: no stub capture can exist.
    #[must_use]
    pub fn source_id(&self) -> &str {
        match *self {}
    }

    /// Unreachable: no stub capture can exist.
    #[must_use]
    pub fn capture_session_generation(&self) -> u64 {
        match *self {}
    }
}
