//! Input-access remediation logic — decides whether the UI owes the user a
//! banner when an interactive effect can't receive host input.
//!
//! Pure and DOM-free so the decision table is unit-testable. The consent
//! gate wins over device denials: until `input.enabled` is on, denied
//! device nodes are expected and not worth surfacing. Browser-preview
//! injection works regardless of host capture, so a healthy-but-idle host
//! pipeline (`devices_opened == 0`, nothing denied) stays silent too.
//!
//! The counters alone could only ever describe Linux. A Windows daemon with no
//! visible window station has zero denied nodes and zero opened ones, which
//! reads as healthy-but-idle — so the typed degradation code is consulted
//! first, and it is what keeps the banner from offering a udev command to a
//! Windows user.

use crate::api::InputStatus;

/// Which remediation the banner should offer, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAccessRemedy {
    /// `input.enabled` is off — offer the one-click consent toggle.
    EnableConsent,
    /// Consent is on but every present input node is unreadable — the
    /// udev-rules / permissions case. Show the install command.
    InstallRules,
    /// The daemon has no interactive desktop to observe. Nothing the user can
    /// fix from here, and no command to run: Raw Input cannot cross a session
    /// boundary, so the answer is to run the foreground daemon in their own
    /// session rather than as a service.
    RunInUserSession,
}

/// Degradation code for a daemon with no visible window station.
const NO_INTERACTIVE_SESSION: &str = "no_interactive_session";

/// Decide the banner state for the active effect.
///
/// Returns `None` unless the active effect actually reacts to input; a
/// non-interactive effect never banners regardless of input health.
#[must_use]
pub fn input_access_remedy(
    effect_wants_input: bool,
    input: &InputStatus,
) -> Option<InputAccessRemedy> {
    if !effect_wants_input {
        return None;
    }
    if !input.enabled {
        return Some(InputAccessRemedy::EnableConsent);
    }
    // Checked before the counter heuristic: on Windows there are no denied
    // nodes to count, so the udev rule would never fire and the user would be
    // left with an interactive effect that silently does nothing.
    if input.degraded.as_deref() == Some(NO_INTERACTIVE_SESSION) {
        return Some(InputAccessRemedy::RunInUserSession);
    }
    if input.devices_denied > 0 && input.devices_opened == 0 {
        return Some(InputAccessRemedy::InstallRules);
    }
    None
}
