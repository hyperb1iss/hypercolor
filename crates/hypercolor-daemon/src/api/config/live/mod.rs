use std::sync::Arc;

use tracing::info;

use hypercolor_types::config_registry::{self, ApplyPolicy, LiveSection};

use crate::app_state::AppState;

mod audio;
mod capture;
mod input;
mod render;

use audio::apply_audio_config_change;
pub(super) use capture::{
    CaptureConfigTransactionError, apply_capture_config_transaction, capture_runtime_matches,
};
#[cfg(test)]
pub(super) use capture::{capture_statuses_match, validate_prepared_capture_status};
pub(super) use input::apply_input_config_change;
use render::apply_render_config_change;
#[cfg(test)]
pub(super) use render::canvas_dimensions_differ;

// ── Registry dispatch ───────────────────────────────────────────────

/// The live subsystems one mutation has to re-apply.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct LiveSections {
    pub(super) audio: bool,
    pub(super) capture: bool,
    pub(super) input: bool,
    pub(super) render: bool,
}

impl LiveSections {
    pub(in crate::api::config) const fn is_empty(self) -> bool {
        !(self.audio || self.capture || self.input || self.render)
    }

    const fn add(&mut self, apply: ApplyPolicy) {
        match apply {
            ApplyPolicy::Live(LiveSection::Audio) => self.audio = true,
            ApplyPolicy::Live(LiveSection::Capture) => self.capture = true,
            ApplyPolicy::Live(LiveSection::Input) => self.input = true,
            ApplyPolicy::Live(LiveSection::Render) => self.render = true,
            ApplyPolicy::LiveOnRead
            | ApplyPolicy::NextScan
            | ApplyPolicy::Restart
            | ApplyPolicy::Inert => {}
        }
    }
}

/// The live subsystems a write at `key` touches, straight from the
/// registry. `None` is a whole-config write, which touches every one.
///
/// The key's own policy comes from its most specific descriptor. Every
/// descriptor nested *below* the key contributes too, because writing a
/// section overwrites the keys inside it: a write to `daemon` carries
/// the render knobs `daemon.target_fps` and friends with it.
pub(super) fn live_sections_for(key: Option<&str>) -> LiveSections {
    let mut sections = LiveSections::default();
    let Some(key) = key else {
        for descriptor in config_registry::registry() {
            sections.add(descriptor.apply);
        }
        return sections;
    };

    sections.add(config_registry::descriptor_for(key).apply);
    let prefix = format!("{key}.");
    for descriptor in config_registry::registry()
        .iter()
        .filter(|descriptor| descriptor.pattern.root().starts_with(&prefix))
    {
        sections.add(descriptor.apply);
    }
    sections
}

/// Whether a write at `key` covers `target` — the same containment the
/// section dispatch uses, narrowed to one knob an applier retunes.
pub(super) fn write_covers(key: Option<&str>, target: &str) -> bool {
    key.is_none_or(|key| key == target || target.starts_with(&format!("{key}.")))
}

/// Re-apply the live sections a write touched.
///
/// Capture is absent by design: its applier is a transaction that
/// persists the config itself, so callers run it before the generic
/// save rather than here.
pub(super) async fn apply_live_sections(
    state: &Arc<AppState>,
    sections: LiveSections,
    key: Option<&str>,
    live_requested: bool,
) -> bool {
    if !live_requested {
        if !sections.is_empty() {
            info!(
                key = key.unwrap_or("<all>"),
                "Persisted config change without live apply; restart the daemon to activate it"
            );
        }
        return false;
    }

    let mut applied = false;
    if sections.audio {
        applied |= apply_audio_config_change(state, key).await;
    }
    if sections.render {
        applied |= apply_render_config_change(state, key).await;
    }
    if sections.input {
        applied |= apply_input_config_change(state, key).await;
    }
    applied
}
