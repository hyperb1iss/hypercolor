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

use hypercolor_types::config::{HypercolorConfig, InteractionRoutePolicy};

use crate::api::{InputSourceStatus, InputStatus};
use crate::ws::InputSourceStatusEventHint;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputStatusEpoch {
    pub connection_generation: u64,
    pub source_event: Option<InputSourceStatusEventHint>,
    pub enabled: bool,
    pub keyboard: bool,
    pub mouse: bool,
    pub daemon_route: InteractionRoutePolicy,
    pub preview_route: InteractionRoutePolicy,
}

#[must_use]
pub fn input_status_epoch(
    connection_generation: u64,
    source_event: Option<InputSourceStatusEventHint>,
    config: Option<&HypercolorConfig>,
) -> Option<InputStatusEpoch> {
    config.map(|config| InputStatusEpoch {
        connection_generation,
        source_event,
        enabled: config.input.enabled,
        keyboard: config.input.keyboard,
        mouse: config.input.mouse,
        daemon_route: config.input.daemon_route,
        preview_route: config.input.preview_route,
    })
}

/// User-facing lifecycle state for the host input pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputPipelineState {
    ConsentOff,
    Live,
    Ready,
    Degraded,
    Unavailable,
}

/// Reduce the structured daemon snapshot to one honest pipeline state.
#[must_use]
pub fn input_pipeline_state(input: &InputStatus) -> InputPipelineState {
    if !input.enabled {
        return InputPipelineState::ConsentOff;
    }
    if input.degraded.is_some() || input.sources.iter().any(source_is_degraded) {
        return InputPipelineState::Degraded;
    }
    if input.host_capturing {
        return InputPipelineState::Live;
    }
    if input.host_capture_registered {
        return InputPipelineState::Ready;
    }
    InputPipelineState::Unavailable
}

/// Best actionable remediation from the aggregate or a source-specific issue.
#[must_use]
pub fn input_status_remediation(input: &InputStatus) -> Option<String> {
    match input_access_remedy(true, input) {
        Some(InputAccessRemedy::EnableConsent) => {
            return Some(
                "Enable host input access to open keyboard or pointer backends.".to_owned(),
            );
        }
        Some(InputAccessRemedy::InstallRules) => {
            return Some(
                "Install the Hypercolor udev rules, then reconnect the input devices.".to_owned(),
            );
        }
        Some(InputAccessRemedy::RunInUserSession) => {
            return Some(
                "Run Hypercolor in your interactive Windows session instead of as a service."
                    .to_owned(),
            );
        }
        None => {}
    }

    input
        .sources
        .iter()
        .filter(|source| source_is_relevant(source))
        .find_map(|source| {
            [
                source.lifecycle_issue.as_ref(),
                source.freshness_issue.as_ref(),
                source.issue.as_ref(),
            ]
            .into_iter()
            .flatten()
            .find_map(|issue| issue.remediation.clone())
        })
}

#[must_use]
pub fn primary_input_source_issue(
    source: &InputSourceStatus,
) -> Option<&crate::api::InputSourceIssueStatus> {
    if matches!(source.state.as_str(), "failed" | "unavailable") {
        source
            .lifecycle_issue
            .as_ref()
            .or(source.issue.as_ref())
            .or(source.freshness_issue.as_ref())
    } else {
        source
            .freshness_issue
            .as_ref()
            .or(source.issue.as_ref())
            .or(source.lifecycle_issue.as_ref())
    }
}

fn source_is_degraded(source: &InputSourceStatus) -> bool {
    source_is_relevant(source)
        && (source.lifecycle_issue.is_some()
            || source.freshness_issue.is_some()
            || source.issue.is_some()
            || matches!(source.state.as_str(), "failed" | "degraded" | "unavailable")
            || (source.demanded && source.freshness == "stale"))
}

// Only host interaction sources speak for the input pipeline. Media,
// network, and audio sources live in their own domains, and an
// unsupported-on-this-platform media source must never degrade input
// health or push its remediation into the input section.
fn source_is_relevant(source: &InputSourceStatus) -> bool {
    source.kind == "interaction"
        && source.backend != "browser"
        && (source.configured || source.demanded)
}

/// Visual tone for the single settings status sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLineTone {
    Active,
    Ready,
    Warn,
}

/// The one sentence the Input section shows, or `None` when the section's
/// own controls already tell the story (consent off, or the permission row
/// is on screen).
#[must_use]
pub fn input_status_line(input: &InputStatus) -> Option<(StatusLineTone, String)> {
    if !input.enabled {
        return None;
    }
    match input_pipeline_state(input) {
        InputPipelineState::ConsentOff => None,
        InputPipelineState::Live => Some((
            StatusLineTone::Active,
            "Capturing input for the active effect.".to_owned(),
        )),
        InputPipelineState::Ready => Some((
            StatusLineTone::Ready,
            "Ready. Capture starts when an effect uses input.".to_owned(),
        )),
        InputPipelineState::Degraded => {
            let sentence = input_status_remediation(input).map_or_else(
                || "Input capture isn't working right now.".to_owned(),
                |remedy| format!("Input capture isn't working right now. {remedy}"),
            );
            Some((StatusLineTone::Warn, sentence))
        }
        InputPipelineState::Unavailable => Some((
            StatusLineTone::Warn,
            "Host input isn't available on this system.".to_owned(),
        )),
    }
}

/// The one sentence the Screen Capture section shows, or `None` when the
/// toggle or the permission row already tells the story.
#[must_use]
pub fn screen_status_line(input: &InputStatus) -> Option<(StatusLineTone, String)> {
    let screen = input
        .sources
        .iter()
        .find(|source| !source.retired && source.kind == "screen")?;

    if screen.state == "live" {
        return Some((StatusLineTone::Active, "Capturing your screen.".to_owned()));
    }
    let issue = primary_input_source_issue(screen);
    if issue.is_some() || matches!(screen.state.as_str(), "failed" | "degraded" | "unavailable") {
        let sentence = issue
            .and_then(|issue| issue.remediation.clone())
            .map_or_else(
                || "Screen capture isn't working right now.".to_owned(),
                |remedy| format!("Screen capture isn't working right now. {remedy}"),
            );
        return Some((StatusLineTone::Warn, sentence));
    }
    Some((
        StatusLineTone::Ready,
        "Ready. Starts when a screen effect runs.".to_owned(),
    ))
}

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
