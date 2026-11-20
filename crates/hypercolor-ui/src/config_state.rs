use std::collections::HashMap;

use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::config_registry::{ApplyPolicy, ConfigKeySchemaEntry, Protection};
use leptos::prelude::*;

use crate::api::ApiError;

#[derive(Clone, Copy)]
pub struct ConfigContext {
    pub config: ReadSignal<Option<HypercolorConfig>>,
    pub set_config: WriteSignal<Option<HypercolorConfig>>,
    pub refresh: Callback<()>,
    pub audio_enabled: Memo<bool>,
}

/// The daemon's config key registry, read once per connection from
/// `GET /api/v1/config/schema`. Every live/restart affordance in the
/// settings surface derives from it rather than from a hand mirror.
#[derive(Clone, Copy)]
pub struct ConfigSchemaContext {
    pub entries: Signal<Vec<ConfigKeySchemaEntry>>,
}

impl ConfigSchemaContext {
    /// Whether a change to `key` only takes effect at the next daemon
    /// start. False while the schema is still in flight, so a badge
    /// appears once the answer is known rather than guessed.
    #[must_use]
    pub fn requires_restart(&self, key: &str) -> bool {
        self.entries
            .with(|entries| schema_requires_restart(entries, key))
    }

    /// Whether the daemon only accepts a write to `key` from a session
    /// that holds a capture credential. False while the schema is still
    /// in flight, so nothing locks on a guess.
    #[must_use]
    pub fn requires_protected_control(&self, key: &str) -> bool {
        self.entries
            .with(|entries| schema_requires_protected_control(entries, key))
    }
}

/// Whether the schema classifies `key` as boot-frozen.
#[must_use]
pub fn schema_requires_restart(entries: &[ConfigKeySchemaEntry], key: &str) -> bool {
    schema_entry_for(entries, key).is_some_and(|entry| matches!(entry.apply, ApplyPolicy::Restart))
}

/// Whether the schema says a write to `key` needs a capture credential.
///
/// Mirrors the daemon's rule over the wire projection: a `tree` entry
/// protects every key under it, and a `section_root` entry protects only
/// a write that addresses the entry itself, so the open leaves of a
/// section whose whole-section write is protected stay open.
#[must_use]
pub fn schema_requires_protected_control(entries: &[ConfigKeySchemaEntry], key: &str) -> bool {
    schema_entry_for(entries, key).is_some_and(|entry| match entry.protection {
        Protection::Open => false,
        Protection::Tree => true,
        Protection::SectionRoot => {
            key == entry.pattern.strip_suffix(".*").unwrap_or(&entry.pattern)
        }
    })
}

/// The most specific schema entry covering `key`.
///
/// Mirrors the daemon's lookup over the wire projection: the deepest
/// matching pattern wins, so an exact key beats the section it sits in,
/// and the `*` catch-all is the last resort. Depth alone settles it
/// because each root — section or `root.*` namespace — appears once.
#[must_use]
pub fn schema_entry_for<'a>(
    entries: &'a [ConfigKeySchemaEntry],
    key: &str,
) -> Option<&'a ConfigKeySchemaEntry> {
    entries
        .iter()
        .filter_map(|entry| pattern_specificity(&entry.pattern, key).map(|rank| (rank, entry)))
        .max_by_key(|(rank, _)| *rank)
        .map(|(_, entry)| entry)
}

/// How specifically `pattern` covers `key`, or `None` when it does not.
/// Deeper patterns win; the catch-all sits below every named pattern.
fn pattern_specificity(pattern: &str, key: &str) -> Option<usize> {
    if pattern == "*" {
        return Some(0);
    }

    let root = pattern.strip_suffix(".*").unwrap_or(pattern);
    if key == root
        || key
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('.'))
    {
        return Some(root.split('.').count());
    }
    None
}

#[derive(Debug, Default)]
pub struct ConfigApplyTracker {
    next_generation: u64,
    current: HashMap<String, u64>,
}

impl ConfigApplyTracker {
    pub fn begin(&mut self, key: &str) -> u64 {
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .expect("config apply generation exhausted");
        self.current.insert(key.to_owned(), self.next_generation);
        self.next_generation
    }

    pub fn finish_if_current(&mut self, key: &str, generation: u64) -> bool {
        if self.current.get(key).copied() != Some(generation) {
            return false;
        }
        self.current.remove(key);
        true
    }
}

#[must_use]
pub fn config_key_value(config: &HypercolorConfig, key: &str) -> Option<serde_json::Value> {
    let root = serde_json::to_value(config).ok()?;
    key.split('.')
        .try_fold(&root, |cursor, part| cursor.get(part))
        .cloned()
}

pub fn apply_config_key(config: &mut HypercolorConfig, key: &str, value: &serde_json::Value) {
    let Ok(mut root) = serde_json::to_value(&*config) else {
        return;
    };

    let parts: Vec<&str> = key.split('.').collect();
    if parts.is_empty() {
        return;
    }

    let (parents, leaf) = parts.split_at(parts.len() - 1);
    let mut cursor = &mut root;
    for &part in parents {
        let Some(obj) = cursor.as_object_mut() else {
            return;
        };
        cursor = obj
            .entry(part.to_owned())
            .or_insert_with(|| serde_json::json!({}));
    }

    if let Some(obj) = cursor.as_object_mut() {
        obj.insert(leaf[0].to_owned(), value.clone());
    }

    if let Ok(updated) = serde_json::from_value(root) {
        *config = updated;
    }
}

// ── Refused writes ──────────────────────────────────────────────────────────

/// A name for the setting behind `key`, for messages about it.
///
/// The capture switches read badly as raw keys ("audio enabled"), so
/// they are named; everything else is its key with the section's own
/// name dropped from a leaf that repeats it.
#[must_use]
pub fn config_setting_label(key: &str) -> String {
    match key {
        "audio" => return "The Audio section".to_owned(),
        "audio.enabled" => return "Audio capture".to_owned(),
        "audio.device" => return "Audio device".to_owned(),
        "capture" => return "The Screen Capture section".to_owned(),
        "capture.enabled" => return "Screen capture".to_owned(),
        "capture.calibration" => return "Capture calibration".to_owned(),
        "input" => return "The Input Access section".to_owned(),
        "input.enabled" => return "Input capture".to_owned(),
        "input.keyboard" => return "Keyboard input".to_owned(),
        "input.mouse" => return "Mouse input".to_owned(),
        _ => {}
    }

    let mut parts = key.split('.');
    let section = parts.next().unwrap_or(key);
    let leaf = parts.next_back().unwrap_or(section);
    let leaf = leaf
        .strip_prefix(section)
        .and_then(|rest| rest.strip_prefix('_'))
        .unwrap_or(leaf);
    let words = if leaf == section {
        section.replace('_', " ")
    } else {
        format!("{} {}", section.replace('_', " "), leaf.replace('_', " "))
    };
    let mut chars = words.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// How a development stack unlocks capture settings in a browser: the
/// dev recipes print a link whose fragment carries the credential. Only
/// debug builds read that fragment, so only they mention it.
pub const DEV_CAPTURE_UNLOCK_HINT: &str =
    "Dev stack: open the protected controls link that just dev prints.";

/// What a config write was trying to do, for the message when it fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigWrite {
    /// Set a key to a new value.
    Change,
    /// Restore a key or section to its default.
    Reset,
}

impl ConfigWrite {
    const fn outcome(self) -> &'static str {
        match self {
            Self::Change => "wasn't changed",
            Self::Reset => "wasn't reset",
        }
    }
}

/// What to tell someone whose write to `key` the daemon refused.
///
/// A capture setting refused for want of a credential gets a sentence
/// about where it can be changed, since the daemon's own wording names
/// machinery a person has no way to act on. Every other refusal carries
/// the daemon's message. `dev_build` adds the one way a development
/// stack hands a browser that credential.
#[must_use]
pub fn config_write_failure_message(
    key: &str,
    write: ConfigWrite,
    error: &ApiError,
    dev_build: bool,
) -> String {
    let label = config_setting_label(key);
    let outcome = write.outcome();
    if is_capture_credential_refusal(key, error) {
        let mut message = format!(
            "{label} {outcome}. Capture settings can only be changed from the Hypercolor app."
        );
        if dev_build {
            message.push(' ');
            message.push_str(DEV_CAPTURE_UNLOCK_HINT);
        }
        return message;
    }

    match error {
        ApiError::Http {
            message: Some(message),
            ..
        } => format!("{label} {outcome}: {message}"),
        other => format!("{label} {outcome}: {other}"),
    }
}

/// Whether `error` is the daemon turning down a capture-setting write
/// because the session holds no capture credential.
#[must_use]
pub fn is_capture_credential_refusal(key: &str, error: &ApiError) -> bool {
    matches!(error, ApiError::Http { status: 403, .. })
        && hypercolor_types::config_registry::requires_protected_control(key)
}

/// Toast the refusal of a write to `key`.
pub fn report_config_write_failure(key: &str, write: ConfigWrite, error: &ApiError) {
    crate::toasts::toast_error(&config_write_failure_message(
        key,
        write,
        error,
        cfg!(debug_assertions),
    ));
}

/// Whether this session is known to be unable to write `key`: the daemon
/// protects the key and the page has no credential to present.
///
/// Read inside a component. The answer follows the daemon connection, so
/// it settles again whenever the socket reopens, which is also when a
/// native shell hands the page its credential. With no schema or socket
/// context the control stays unlocked and a refusal is reported instead.
#[must_use]
pub fn protected_write_locked(key: &str) -> Signal<bool> {
    let key = key.to_owned();
    let schema = use_context::<ConfigSchemaContext>();
    let connection_generation =
        use_context::<crate::app::WsContext>().map(|ws| ws.connection_generation);
    Signal::derive(move || {
        if let Some(connection_generation) = connection_generation {
            connection_generation.track();
        }
        schema.is_some_and(|schema| schema.requires_protected_control(&key))
            && !crate::api::client::may_hold_protected_control()
    })
}

/// The line a settings section shows above its locked rows. `subject`
/// starts the sentence ("Screen capture settings"), and `dev_build` adds
/// the same hint the refusal toast carries, since a locked row never
/// reaches that toast.
#[must_use]
pub fn capture_lock_notice(subject: &str, dev_build: bool) -> String {
    let mut notice =
        format!("{subject} are locked in this browser. Change them from the Hypercolor app.");
    if dev_build {
        notice.push(' ');
        notice.push_str(DEV_CAPTURE_UNLOCK_HINT);
    }
    notice
}

/// Whether this session can write none of the capture settings, for the
/// one-line notice a settings section shows above its locked rows.
#[must_use]
pub fn capture_settings_locked() -> Signal<bool> {
    protected_write_locked("capture.enabled")
}
