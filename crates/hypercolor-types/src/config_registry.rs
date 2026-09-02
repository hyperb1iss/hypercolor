//! The declarative config-key registry (Spec 76 §3.3).
//!
//! One table answers, for every config key: how a change takes effect,
//! how the value renders on read surfaces, and how it validates. The
//! daemon's live-apply dispatch, restart reporting, and redaction all
//! derive from this table (wave 4.3 deletes the four hand predicates
//! and the UI's hand mirror); clients read it via `GET /config/schema`.
//!
//! Granularity is per section, with exact-key overrides where one
//! section genuinely mixes behaviors (the `daemon` section holds both
//! live render knobs and boot-frozen listener settings). Lookup picks
//! the most specific match: exact key, then closed section, then
//! dynamic namespace, then the flattened-extensions catch-all.
//!
//! [`ApplyPolicy`] names five honest behaviors observed in the tree,
//! not three idealized ones: keys the daemon actively re-applies
//! ([`ApplyPolicy::Live`]), keys read fresh on every use with no
//! dispatch needed ([`ApplyPolicy::LiveOnRead`]), keys the discovery
//! worker picks up on its next tick ([`ApplyPolicy::NextScan`]), keys
//! frozen at startup ([`ApplyPolicy::Restart`]), and keys the daemon
//! parses and persists but does not read today
//! ([`ApplyPolicy::Inert`]).

use serde::{Deserialize, Serialize};

/// What a descriptor's pattern matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPattern {
    /// One exact dotted key (`"daemon.target_fps"`).
    Exact(&'static str),
    /// A closed struct section: the root itself and every key under it
    /// (`"audio"` matches `audio` and `audio.device`).
    Section(&'static str),
    /// A dynamic map namespace with caller-defined keys
    /// (`"drivers"` matches `drivers.wled.known_ips`). Redaction for
    /// namespaces is deny-by-default.
    Namespace(&'static str),
    /// The flattened-extensions catch-all: any top-level section the
    /// build does not model. Always the least specific match.
    ExtensionsCatchAll,
}

impl KeyPattern {
    /// The pattern's root segment (empty for the catch-all).
    #[must_use]
    pub const fn root(&self) -> &'static str {
        match self {
            Self::Exact(key) => key,
            Self::Section(root) | Self::Namespace(root) => root,
            Self::ExtensionsCatchAll => "",
        }
    }

    /// Whether `key` (a dotted path, or a bare section name) matches.
    #[must_use]
    pub fn matches(&self, key: &str) -> bool {
        match self {
            Self::Exact(exact) => key == *exact,
            Self::Section(root) | Self::Namespace(root) => {
                key == *root
                    || (key.len() > root.len()
                        && key.starts_with(root)
                        && key.as_bytes()[root.len()] == b'.')
            }
            Self::ExtensionsCatchAll => true,
        }
    }

    /// Specificity rank for lookup (higher wins).
    const fn specificity(&self) -> u8 {
        match self {
            Self::Exact(_) => 3,
            Self::Section(_) => 2,
            Self::Namespace(_) => 1,
            Self::ExtensionsCatchAll => 0,
        }
    }
}

/// The daemon subsystem a live-applied key re-configures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum LiveSection {
    /// Audio input pipeline rebuild.
    Audio,
    /// Screen-capture transaction (prepare/validate/persist/commit).
    Capture,
    /// Host input routing and capture sources.
    Input,
    /// Render loop FPS tier and canvas dimensions.
    Render,
}

/// How a change to a key takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case", tag = "kind", content = "section")]
pub enum ApplyPolicy {
    /// The daemon actively re-applies the change through the named
    /// subsystem's dispatch.
    Live(LiveSection),
    /// Read fresh on every use — takes effect on the next read with no
    /// dispatch (session policy, per-scene media admission, driver
    /// settings).
    LiveOnRead,
    /// The discovery worker resolves it on its next tick.
    NextScan,
    /// Frozen at startup; takes effect at the next daemon start.
    Restart,
    /// Parsed and persisted, but nothing in the daemon reads it today.
    Inert,
}

/// How a key's value renders on read surfaces (config GET, schema,
/// events).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Redaction {
    /// Rendered verbatim.
    Plain,
    /// Masked on read surfaces. Dynamic namespaces default here —
    /// deny-by-default — with driver-declared metadata selectively
    /// relaxing individual keys at runtime (wave 4.3).
    Secret,
}

/// Which writes to a row need a protected-control credential.
///
/// Protected controls start or retarget a consented capture stream,
/// so they stay behind an API key even on a keyless install; tuning an
/// already-consented stream does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Protection {
    /// Any caller with config write access may set it.
    Open,
    /// Writing the whole section is protected; its leaves decide for
    /// themselves through more specific rows.
    SectionRoot,
    /// The row and every key beneath it are protected.
    Tree,
}

/// Extra validation a descriptor runs beyond the serde round-trip.
pub type KeyValidator = fn(&serde_json::Value) -> Result<(), String>;

/// One registry row.
pub struct ConfigKeyDescriptor {
    /// What this row matches.
    pub pattern: KeyPattern,
    /// How changes take effect.
    pub apply: ApplyPolicy,
    /// Extra validation beyond the serde round-trip the set path
    /// already performs. `None` means the type system is the check.
    pub validate: Option<KeyValidator>,
    /// Read-surface rendering.
    pub redaction: Redaction,
    /// Which writes need a protected-control credential.
    pub protection: Protection,
}

/// Declares the registry table plus the closed-section root list the
/// completeness test checks against the real config struct.
///
/// Two constraints the tests rely on: top-level config sections must
/// not be `skip_serializing_if` (the completeness check enumerates the
/// serialized default's keys; `extensions` is the one exception and is
/// the declared catch-all), and exact-key rows carry no validator and
/// `Plain` redaction — an exact override exists to change the apply
/// policy only. Grow the macro when an exact row first needs more.
macro_rules! config_key_registry {
    (
        sections: [ $( $sroot:literal => $sapply:expr, $sredact:expr $(, validate: $svalidate:expr)? $(, protect: $sprotect:expr)? ; )* ]
        exact: [ $( $ekey:literal => $eapply:expr $(, protect: $eprotect:expr)? ; )* ]
        namespaces: [ $( $nroot:literal => $napply:expr ; )* ]
    ) => {
        /// Every registry row, most-specific rows not required to come
        /// first — lookup ranks by pattern specificity.
        #[must_use]
        pub fn registry() -> &'static [ConfigKeyDescriptor] {
            static REGISTRY: &[ConfigKeyDescriptor] = &[
                $(ConfigKeyDescriptor {
                    pattern: KeyPattern::Section($sroot),
                    apply: $sapply,
                    validate: config_key_registry!(@validate $($svalidate)?),
                    redaction: $sredact,
                    protection: config_key_registry!(@protect $($sprotect)?),
                },)*
                $(ConfigKeyDescriptor {
                    pattern: KeyPattern::Exact($ekey),
                    apply: $eapply,
                    validate: None,
                    redaction: Redaction::Plain,
                    protection: config_key_registry!(@protect $($eprotect)?),
                },)*
                $(ConfigKeyDescriptor {
                    pattern: KeyPattern::Namespace($nroot),
                    apply: $napply,
                    validate: None,
                    redaction: Redaction::Secret,
                    protection: Protection::Open,
                },)*
                ConfigKeyDescriptor {
                    pattern: KeyPattern::ExtensionsCatchAll,
                    apply: ApplyPolicy::Inert,
                    validate: None,
                    redaction: Redaction::Secret,
                    protection: Protection::Open,
                },
            ];
            REGISTRY
        }

        /// The closed-section roots the registry declares — the
        /// completeness test asserts these cover the config struct.
        #[must_use]
        pub fn declared_section_roots() -> &'static [&'static str] {
            &[$($sroot,)*]
        }

        /// The dynamic-namespace roots the registry declares.
        #[must_use]
        pub fn declared_namespace_roots() -> &'static [&'static str] {
            &[$($nroot,)*]
        }
    };
    (@validate $validate:expr) => { Some($validate) };
    (@validate) => { None };
    (@protect $protect:expr) => { $protect };
    (@protect) => { Protection::Open };
}

fn validate_capture_section(value: &serde_json::Value) -> Result<(), String> {
    let capture: crate::config::CaptureConfig =
        serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
    capture.validate().map_err(|error| error.to_string())
}

config_key_registry! {
    sections: [
        // schema_version is migration-managed; hand edits take effect
        // only through the migration path at next load.
        "schema_version" => ApplyPolicy::Restart, Redaction::Plain;
        "daemon" => ApplyPolicy::Restart, Redaction::Plain;
        "web" => ApplyPolicy::Restart, Redaction::Plain;
        "mcp" => ApplyPolicy::Restart, Redaction::Plain;
        "effect_engine" => ApplyPolicy::Restart, Redaction::Plain;
        "rendering" => ApplyPolicy::Restart, Redaction::Plain;
        "media" => ApplyPolicy::LiveOnRead, Redaction::Plain;
        // Whole-section writes can retarget a consented stream; DSP
        // tuning leaves stay open so a keyless install keeps its sliders.
        "audio" => ApplyPolicy::Live(LiveSection::Audio), Redaction::Plain,
            protect: Protection::SectionRoot;
        "capture" => ApplyPolicy::Live(LiveSection::Capture), Redaction::Plain,
            validate: validate_capture_section, protect: Protection::Tree;
        "input" => ApplyPolicy::Live(LiveSection::Input), Redaction::Plain,
            protect: Protection::SectionRoot;
        // The running render and display threads freeze their copies;
        // the API reading live is reporting, not application.
        "display" => ApplyPolicy::Restart, Redaction::Plain;
        // web.cors_origins is mixed in the tree (HTTP layer frozen,
        // WS handshake live); Restart is the conservative truth for
        // the section until the HTTP layer re-reads.
        "discovery" => ApplyPolicy::NextScan, Redaction::Plain;
        "network" => ApplyPolicy::Restart, Redaction::Plain;
        "session" => ApplyPolicy::LiveOnRead, Redaction::Plain;
    ]
    exact: [
        "daemon.target_fps" => ApplyPolicy::Live(LiveSection::Render);
        "daemon.canvas_width" => ApplyPolicy::Live(LiveSection::Render);
        "daemon.canvas_height" => ApplyPolicy::Live(LiveSection::Render);
        // stream_private_network_allowlist bakes into the AssetLibrary
        // at startup, unlike the rest of its LiveOnRead section.
        "media.stream_private_network_allowlist" => ApplyPolicy::Restart;
        // Module construction and worker spawn freeze these three,
        // unlike the rest of their NextScan section.
        "discovery.mdns_enabled" => ApplyPolicy::Restart;
        "discovery.background_enabled" => ApplyPolicy::Restart;
        "discovery.scan_interval_secs" => ApplyPolicy::Restart;
        // The session watcher only spawns at boot: enabling after a
        // disabled start needs a restart even though the policy reads
        // live once running. Restart is the conservative truth.
        "session.enabled" => ApplyPolicy::Restart;
        // Read from the manager on every effect-error event, unlike
        // the rest of its boot-frozen section.
        "effect_engine.effect_error_fallback" => ApplyPolicy::LiveOnRead;
        // Starting or retargeting a capture source is a consent act.
        "audio.enabled" => ApplyPolicy::Live(LiveSection::Audio), protect: Protection::Tree;
        "audio.device" => ApplyPolicy::Live(LiveSection::Audio), protect: Protection::Tree;
        "input.enabled" => ApplyPolicy::Live(LiveSection::Input), protect: Protection::Tree;
        "input.keyboard" => ApplyPolicy::Live(LiveSection::Input), protect: Protection::Tree;
        "input.mouse" => ApplyPolicy::Live(LiveSection::Input), protect: Protection::Tree;
    ]
    namespaces: [
        "drivers" => ApplyPolicy::LiveOnRead;
    ]
}

/// Whether `key` is a well-formed dotted path: non-empty, with no
/// empty segments. Lookup itself is total (the catch-all matches
/// anything), so surfaces that accept caller-supplied keys gate on
/// this first (wave 4.3's set path).
#[must_use]
pub fn is_valid_key(key: &str) -> bool {
    !key.is_empty() && key.split('.').all(|segment| !segment.is_empty())
}

/// The most specific descriptor matching `key`. Total: the
/// extensions catch-all matches anything.
#[must_use]
pub fn descriptor_for(key: &str) -> &'static ConfigKeyDescriptor {
    registry()
        .iter()
        .filter(|descriptor| descriptor.pattern.matches(key))
        .max_by_key(|descriptor| descriptor.pattern.specificity())
        .expect("the extensions catch-all matches every key")
}

/// Whether a change to `key` requires a daemon restart to take effect.
#[must_use]
pub fn requires_restart(key: &str) -> bool {
    matches!(descriptor_for(key).apply, ApplyPolicy::Restart)
}

/// Whether writing `key` needs a protected-control credential.
#[must_use]
pub fn requires_protected_control(key: &str) -> bool {
    let descriptor = descriptor_for(key);
    match (descriptor.protection, descriptor.pattern) {
        (Protection::Open, _) => false,
        (Protection::Tree, _) => true,
        (Protection::SectionRoot, KeyPattern::Section(root)) => key == root,
        (Protection::SectionRoot, _) => true,
    }
}

/// Whether `key` is masked on read surfaces.
#[must_use]
pub fn is_redacted(key: &str) -> bool {
    matches!(descriptor_for(key).redaction, Redaction::Secret)
}

/// Mask `value`, read at `key`, the way every read surface renders it.
///
/// Plain keys pass through untouched. A secret dynamic namespace read
/// at its root masks per entry, so a client still learns which driver
/// blocks exist; every other secret read collapses to the marker.
/// Config reads, key reads, and the `ConfigChanged` event share this
/// one rule, so a credential renders the same way on every surface.
#[must_use]
pub fn redact_value(key: &str, value: serde_json::Value) -> serde_json::Value {
    let descriptor = descriptor_for(key);
    if !matches!(descriptor.redaction, Redaction::Secret) {
        return value;
    }
    if key == descriptor.pattern.root()
        && let KeyPattern::Namespace(_) = descriptor.pattern
        && let Some(entries) = value.as_object()
    {
        return serde_json::Value::Object(
            entries
                .keys()
                .map(|entry| (entry.clone(), redacted_marker()))
                .collect(),
        );
    }
    redacted_marker()
}

/// The marker every masked value renders as.
#[must_use]
pub fn redacted_marker() -> serde_json::Value {
    serde_json::json!({ "redacted": true })
}

/// One schema row as served to clients (`GET /config/schema`,
/// wave 4.3) — the wire projection of a descriptor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ConfigKeySchemaEntry {
    /// The pattern text: an exact key, a section root, a namespace
    /// root suffixed `.*`, or `*` for the extensions catch-all.
    pub pattern: String,
    /// How changes take effect.
    pub apply: ApplyPolicy,
    /// Read-surface rendering.
    pub redaction: Redaction,
    /// Whether the daemon runs extra validation beyond type checking.
    pub has_validator: bool,
    /// Which writes need a protected-control credential.
    #[serde(default = "default_protection")]
    pub protection: Protection,
}

const fn default_protection() -> Protection {
    Protection::Open
}

impl From<&ConfigKeyDescriptor> for ConfigKeySchemaEntry {
    fn from(descriptor: &ConfigKeyDescriptor) -> Self {
        let pattern = match descriptor.pattern {
            KeyPattern::Exact(key) => key.to_owned(),
            KeyPattern::Section(root) => root.to_owned(),
            KeyPattern::Namespace(root) => format!("{root}.*"),
            KeyPattern::ExtensionsCatchAll => "*".to_owned(),
        };
        Self {
            pattern,
            apply: descriptor.apply,
            redaction: descriptor.redaction,
            has_validator: descriptor.validate.is_some(),
            protection: descriptor.protection,
        }
    }
}

/// The full schema payload.
#[must_use]
pub fn schema_entries() -> Vec<ConfigKeySchemaEntry> {
    registry().iter().map(ConfigKeySchemaEntry::from).collect()
}
