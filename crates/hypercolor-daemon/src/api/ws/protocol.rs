//! WebSocket protocol types — subscriptions, configs, and client/server messages.
//!
//! These types describe the wire format on `/api/v1/ws`. Everything here is data —
//! no network I/O, no caches, no runtime state.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::hash::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::de::{self, IgnoredAny, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::json;

use hypercolor_leptos_ext::ws::registry::{
    CanvasConfig, CanvasFormat, FramesConfig, TopicId, TopicSet,
};
use hypercolor_leptos_ext::ws::topic::{
    ActiveSubscription, PatchError, SubscriptionTable, TopicSelector, TopicSubscription,
};
use hypercolor_leptos_ext::ws::{
    DEFAULT_PREVIEW_MAX_DECODED_PUBLICATION_BYTES, INTERACTIVE_PREVIEW_ID_MAX_BYTES,
};
use hypercolor_types::canvas::SurfaceDescriptor;
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::server::ServerIdentity;
use hypercolor_types::spatial::SpatialLayout;

use crate::device_metrics::DeviceMetricsSnapshot;
use crate::domain::DomainError;

// ── Subscription Types ───────────────────────────────────────────────────

/// One validated wire selector: the topic a client named plus the
/// canonical key its key type parsed. Unkeyed topics carry `None`; keyed
/// ones carry whatever their key type accepted, never the raw client
/// text, because the boundary — not the caller — decides what a key is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TopicSelection {
    pub(super) topic: TopicId,
    pub(super) key: Option<String>,
}

/// One validated subscribe entry: a selection plus the config patch that
/// travelled with it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SubscriptionRequest {
    pub(super) selection: TopicSelection,
    pub(super) config: Option<serde_json::Value>,
}

/// One connection's live subscriptions.
///
/// Membership and per-subscription config are two views of one fact, so
/// they move together: [`SubscriptionState::admit`] is the only place a
/// topic joins the set, and it materializes that topic's default config
/// in the same step. Every client-visible change goes through
/// [`SubscriptionState::subscribe`] or
/// [`SubscriptionState::unsubscribe`], which build a whole replacement
/// state the caller swaps in only after the runtime accepts it.
///
/// Config outlives membership on purpose: unsubscribing drops the topic
/// from the set, and its config moves aside into [`DormantConfigs`] so a
/// client that re-subscribes gets its own settings back rather than the
/// defaults. The live table only ever holds live subscriptions, which is
/// what its own contract promises and what `any_for` has to keep meaning.
#[derive(Debug, Clone)]
pub(super) struct SubscriptionState {
    topics: TopicSet,
    live: SubscriptionTable,
    dormant: DormantConfigs,
}

impl Default for SubscriptionState {
    /// A fresh connection starts subscribed to `events` and nothing else.
    fn default() -> Self {
        let mut state = Self {
            topics: TopicSet::EMPTY,
            live: SubscriptionTable::default(),
            dormant: DormantConfigs::default(),
        };
        state.admit(TopicId::Events, None);
        state
    }
}

/// One live subscription, as the relays and the acknowledgment read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LiveSubscription<'a> {
    pub(super) topic: TopicId,
    pub(super) key: Option<&'a str>,
    pub(super) config: &'a serde_json::Value,
}

/// Config a client set for a topic it is not currently subscribed to.
///
/// Kept apart from the live subscription table on purpose: that table
/// means "subscribed", and a config that outlives its subscription would
/// make it lie. Keyed the same way, so the two halves stay swappable as
/// keyed topics arrive.
#[derive(Debug, Clone, Default)]
struct DormantConfigs(BTreeMap<(u32, Option<String>), serde_json::Value>);

impl DormantConfigs {
    fn get(&self, bit: u32, key: Option<&str>) -> Option<&serde_json::Value> {
        self.0.get(&(bit, key.map(str::to_owned)))
    }

    fn insert(&mut self, bit: u32, key: Option<String>, config: serde_json::Value) {
        self.0.insert((bit, key), config);
    }

    fn take(&mut self, bit: u32, key: Option<&str>) -> Option<serde_json::Value> {
        self.0.remove(&(bit, key.map(str::to_owned)))
    }
}

impl SubscriptionState {
    pub(super) const fn topics(&self) -> TopicSet {
        self.topics
    }

    pub(super) const fn contains(&self, topic: TopicId) -> bool {
        self.topics.contains(topic)
    }

    /// One subscription's config, live or dormant, or the topic's default
    /// when the client has never configured that key. Dormant counts so a
    /// client keeps its own settings across unsubscribe and resubscribe.
    pub(super) fn config_of<C>(&self, topic: TopicId, key: Option<&str>) -> C
    where
        C: serde::de::DeserializeOwned + Default,
    {
        match self.stored_config(topic.bit(), key) {
            // Borrowed, not cloned: relays re-read config on every frame
            // they pace.
            Some(stored) => C::deserialize(stored)
                .expect("stored topic config round-trips through its own config type"),
            None => C::default(),
        }
    }

    /// Every live key of a keyed topic with its typed config, in key
    /// order. Relays that fan out across keys walk this.
    pub(super) fn keyed_configs<C>(&self, topic: TopicId) -> Vec<(String, C)>
    where
        C: serde::de::DeserializeOwned,
    {
        self.live
            .entries_for(topic.bit())
            .filter_map(|(key, config)| {
                let key = key?.to_owned();
                let config = C::deserialize(config)
                    .expect("stored topic config round-trips through its own config type");
                Some((key, config))
            })
            .collect()
    }

    fn stored_config(&self, bit: u32, key: Option<&str>) -> Option<&serde_json::Value> {
        self.live
            .config(bit, key)
            .or_else(|| self.dormant.get(bit, key))
    }

    /// Whether one specific subscription is live.
    #[cfg(test)]
    pub(super) fn holds(&self, topic: TopicId, key: Option<&str>) -> bool {
        self.live.config(topic.bit(), key).is_some()
    }

    /// Every live subscription, topic declaration order then key order.
    pub(super) fn live_subscriptions(&self) -> impl Iterator<Item = LiveSubscription<'_>> {
        TopicId::ALL.iter().copied().flat_map(move |topic| {
            self.live
                .entries_for(topic.bit())
                .map(move |(key, config)| LiveSubscription { topic, key, config })
        })
    }

    /// The subscription snapshot every acknowledgment carries. Configless
    /// topics report no `config` at all rather than a bare `null`.
    pub(super) fn projection(&self) -> Vec<ActiveSubscription> {
        self.live_subscriptions()
            .map(|live| ActiveSubscription {
                topic: live.topic.as_str().to_owned(),
                key: live.key.map(str::to_owned),
                config: (!live.config.is_null()).then(|| live.config.clone()),
                publication_id: None,
            })
            .collect()
    }

    /// Build the state a subscribe request would produce.
    ///
    /// The whole request is one transaction: every entry joins, its config
    /// patch applies against the subscription it named, and every runtime
    /// admission runs on a candidate copy. Any failure returns the error
    /// with the live state untouched, so a request that names four
    /// subscriptions and mis-configures the fourth changes nothing.
    pub(super) fn subscribe(
        &self,
        requests: &[SubscriptionRequest],
    ) -> Result<Self, WsProtocolError> {
        let mut next = self.clone();
        for request in requests {
            next.admit(request.selection.topic, request.selection.key.clone());
        }
        // Request order, so a client that sends two bad patches always
        // hears about the first one it wrote.
        for request in requests {
            let Some(patch) = request.config.as_ref() else {
                continue;
            };
            // A null patch on a topic that takes config means "no patch",
            // exactly as an absent one does. Configless topics still go
            // to the vtable, which refuses null on apply.
            if patch.is_null() && request.selection.topic.vtable().configurable {
                continue;
            }
            next.apply_patch(
                request.selection.topic,
                request.selection.key.as_deref(),
                patch,
            )?;
        }

        Ok(next)
    }

    /// Build the state an unsubscribe request would produce. Stored
    /// config moves aside rather than dying, so a later re-subscribe
    /// reinstates it.
    pub(super) fn unsubscribe(&self, selections: &[TopicSelection]) -> Self {
        let mut next = self.clone();
        for selection in selections {
            next.retire(selection.topic, selection.key.as_deref());
        }
        next
    }

    /// The single write path for joining: the set gains the topic and
    /// the live table gains that key's config in the same step.
    ///
    /// Configless topics store a `null` config, so "has a live entry" and
    /// "is subscribed" mean the same thing for every topic and the table
    /// alone answers membership questions per key.
    fn admit(&mut self, topic: TopicId, key: Option<String>) {
        self.topics.insert(topic);
        let bit = topic.bit();
        if self.live.config(bit, key.as_deref()).is_some() {
            return;
        }
        let config = self
            .dormant
            .take(bit, key.as_deref())
            .unwrap_or_else(|| (topic.vtable().default_config_json)());
        self.live.insert(bit, key, config);
    }

    /// The single write path for leaving: the live table loses that key
    /// and its config moves to the dormant cache in the same step. The
    /// topic leaves the set only once its last key is gone.
    fn retire(&mut self, topic: TopicId, key: Option<&str>) {
        let bit = topic.bit();
        if let Some(config) = self.live.config(bit, key).cloned() {
            self.live.remove(bit, key);
            if !config.is_null() {
                self.dormant.insert(bit, key.map(str::to_owned), config);
            }
        }
        if !self.live.any_for(bit) {
            self.topics.remove(topic);
        }
    }

    fn apply_patch(
        &mut self,
        topic: TopicId,
        key: Option<&str>,
        patch: &serde_json::Value,
    ) -> Result<(), WsProtocolError> {
        let bit = topic.bit();
        let current = self
            .stored_config(bit, key)
            .cloned()
            .unwrap_or_else(|| (topic.vtable().default_config_json)());
        let next = (topic.vtable().apply_patch_json)(&current, patch)
            .map_err(|error| config_patch_error(topic, &error))?;
        super::topics::admit_config(topic, &next)?;
        // Every patch arrives attached to a selector the same request
        // admitted, so the live entry for this key always exists by now.
        debug_assert!(
            self.live.config(bit, key).is_some(),
            "patch target must already be live for its key"
        );
        self.live.insert(bit, key.map(str::to_owned), next);
        Ok(())
    }
}

#[cfg(test)]
impl SubscriptionState {
    /// Whether the live table still means what its name says: a topic has
    /// at least one live entry exactly when it is in the membership set.
    pub(super) fn live_table_agrees_with_membership(&self) -> bool {
        TopicId::ALL
            .iter()
            .copied()
            .all(|topic| self.live.any_for(topic.bit()) == self.topics.contains(topic))
    }

    /// Whether this subscription's config is parked for a re-subscribe.
    pub(super) fn has_dormant_config(&self, topic: TopicId, key: Option<&str>) -> bool {
        self.dormant.get(topic.bit(), key).is_some()
    }

    /// Drive one subscribe request the way the wire drives it: wire
    /// entries in, the same parse, transaction, and admission out.
    pub(super) fn subscribed(
        &self,
        entries: Vec<TopicSubscription>,
    ) -> Result<Self, WsProtocolError> {
        self.subscribe(&parse_subscriptions(&entries)?)
    }

    /// Subscribe to unkeyed topics, pulling each one's config out of a
    /// map keyed by topic name. A test convenience: the wire itself
    /// carries config inside each selector, which is what makes a patch
    /// for a topic the request never named unrepresentable.
    pub(super) fn subscribed_unkeyed(
        &self,
        topics: &[&str],
        config: serde_json::Value,
    ) -> Result<Self, WsProtocolError> {
        let entries = topics
            .iter()
            .map(|topic| TopicSubscription {
                topic: (*topic).to_owned(),
                key: None,
                config: config.get(*topic).cloned(),
            })
            .collect();
        self.subscribed(entries)
    }

    /// Drive one unsubscribe request the same way.
    pub(super) fn unsubscribed(&self, selectors: Vec<TopicSelector>) -> Self {
        let selections = parse_selectors(&selectors).expect("test selectors parse");
        self.unsubscribe(&selections)
    }

    /// Unsubscribe from unkeyed topics by name.
    pub(super) fn unsubscribed_unkeyed(&self, topics: &[&str]) -> Self {
        self.unsubscribed(
            topics
                .iter()
                .map(|topic| TopicSelector::unkeyed(*topic))
                .collect(),
        )
    }

    /// The live configs viewed as `{topic: config}` for unkeyed topics and
    /// `{topic: {key: config}}` for keyed ones. A test-shaped view of
    /// [`Self::projection`], which is what the wire actually carries.
    pub(super) fn config_by_topic(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for live in self.live_subscriptions() {
            if live.config.is_null() {
                continue;
            }
            match live.key {
                None => {
                    map.insert(live.topic.as_str().to_owned(), live.config.clone());
                }
                Some(key) => {
                    let entry = map
                        .entry(live.topic.as_str().to_owned())
                        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
                    if let Some(keyed) = entry.as_object_mut() {
                        keyed.insert(key.to_owned(), live.config.clone());
                    }
                }
            }
        }
        serde_json::Value::Object(map)
    }
}

/// Project a rejected patch onto the wire's error vocabulary.
///
/// A configless topic refuses any non-null config stanza.
/// Field-level rejections name the field under its topic; whole-value
/// rejections name the topic alone.
fn config_patch_error(topic: TopicId, error: &PatchError) -> WsProtocolError {
    if !topic.vtable().configurable {
        return WsProtocolError::invalid_config(
            format!("config.{}", topic.as_str()),
            "topic accepts no config",
        );
    }

    let field = match error.field {
        "config" | "patch" => format!("config.{}", topic.as_str()),
        field => format!("config.{}.{field}", topic.as_str()),
    };
    WsProtocolError::invalid_config(field, error.reason.clone())
}

#[derive(Debug, Clone)]
pub enum FrameZoneSelection {
    All,
    Named(HashSet<String>),
}

impl FrameZoneSelection {
    pub fn new(selected: &[String]) -> Self {
        if selected.iter().any(|zone| zone == "all") {
            Self::All
        } else {
            Self::Named(selected.iter().cloned().collect())
        }
    }

    #[cfg(test)]
    pub(super) fn select<'a>(
        &self,
        zones: &'a [hypercolor_types::event::ZoneColors],
    ) -> Vec<&'a hypercolor_types::event::ZoneColors> {
        match self {
            Self::All => zones.iter().collect(),
            Self::Named(_) => zones
                .iter()
                .filter(|zone| self.includes(zone.zone_id.as_str()))
                .collect(),
        }
    }

    pub(super) fn includes(&self, zone_id: &str) -> bool {
        match self {
            Self::All => true,
            Self::Named(selected) => selected.contains(zone_id),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct ActiveFramesConfig {
    pub(super) config: FramesConfig,
    pub(super) selection_hash: u64,
    pub(super) selection: FrameZoneSelection,
}

impl ActiveFramesConfig {
    pub(super) fn new(config: FramesConfig) -> Self {
        let selection_hash = frame_selection_hash(&config.zones);
        let selection = FrameZoneSelection::new(&config.zones);
        Self {
            config,
            selection_hash,
            selection,
        }
    }
}

pub(super) fn validate_passive_preview_shape(
    config: &CanvasConfig,
    field: impl Into<String>,
) -> Result<(), WsProtocolError> {
    if config.width == 0 || config.height == 0 {
        return Ok(());
    }
    validate_preview_surface_resource(config.width, config.height, config.format)
        .map(|_| ())
        .map_err(|reason| {
            WsProtocolError::invalid_config_resource(field, config.width, config.height, reason)
        })
}

/// Hard transport ceiling for one complete WebSocket message or frame.
pub(crate) const MAX_WS_MESSAGE_BYTES: usize = 1024 * 1024;
/// Maximum decoded surface bytes admitted for one preview publication.
pub(crate) const MAX_PREVIEW_PUBLICATION_BYTES: usize =
    DEFAULT_PREVIEW_MAX_DECODED_PUBLICATION_BYTES;
/// Maximum number of edges accepted in one browser-input batch.
pub(super) const MAX_INPUT_INJECT_EVENTS: usize = 256;
/// Maximum UTF-8 byte length of an injected key or button name.
pub(super) const MAX_INPUT_NAME_BYTES: usize = 128;
/// Largest accepted browser wheel delta, equivalent to 100 notches.
pub(super) const MAX_INPUT_WHEEL_DELTA: i32 = 120 * 100;
/// Largest accepted exact browser scroll delta on either axis.
pub(super) const MAX_INPUT_SCROLL_Q16_16: i64 = (120_i64 * 100) << 16;

macro_rules! define_client_messages {
    ($($(#[$meta:meta])* $variant:ident $body:tt),+ $(,)?) => {
        /// Client-to-server subscription messages.
        #[derive(Debug, Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
        pub(super) enum ClientMessage {
            $($(#[$meta])* $variant $body),+
        }

        pub(super) fn client_message_vocabulary() -> Vec<String> {
            vec![$(hypercolor_types::event::pascal_to_snake_case(stringify!($variant))),+]
        }
    };
}

define_client_messages! {
    /// Subscribe to one or more topics.
    ///
    /// Each entry names a topic, its key when the topic is keyed, and an
    /// optional config patch. Config rides with its selector, so a patch
    /// can only ever target a subscription the same request establishes,
    /// and the topic that owns the config validates it through the
    /// registry vtable.
    Subscribe { topics: Vec<TopicSubscription> },
    /// Unsubscribe from one or more topics.
    Unsubscribe { topics: Vec<TopicSelector> },
    /// REST-equivalent command execution over WS.
    Command {
        id: String,
        method: String,
        path: String,
        #[serde(default)]
        body: Option<serde_json::Value>,
    },
    /// Transient per-zone layout preview for Studio drag interactions.
    ///
    /// Active-scene-only and zone-keyed: fine-grained mutation is
    /// live-tree-only across every transport, so there is no scene to
    /// select (Spec 78 §1.5).
    ZoneLayoutPreview {
        zone_id: String,
        layout: SpatialLayout,
    },
    /// Clear one transient per-zone layout preview.
    ZoneLayoutPreviewClear { zone_id: String },
    /// Inject browser-preview input edges into one active preview.
    InputInject {
        #[serde(deserialize_with = "deserialize_interactive_preview_id")]
        preview_id: String,
        #[serde(deserialize_with = "deserialize_input_edges")]
        events: Vec<BrowserInputEdgeWire>,
    },
    /// Claim this preview as the daemon's authoritative browser input.
    InteractivePreviewClaimAuthoritative {
        #[serde(deserialize_with = "deserialize_interactive_preview_id")]
        preview_id: String,
    },
    /// Release this preview's authoritative browser-input claim.
    InteractivePreviewReleaseAuthoritative {
        #[serde(deserialize_with = "deserialize_interactive_preview_id")]
        preview_id: String,
    },
}

/// Wire form of one injected input edge from a browser preview.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum BrowserInputEdgeWire {
    Key {
        #[serde(deserialize_with = "deserialize_input_name")]
        key: String,
        state: InputButtonStateWire,
    },
    Button {
        #[serde(deserialize_with = "deserialize_input_button")]
        button: String,
        state: InputButtonStateWire,
    },
    Move {
        #[serde(deserialize_with = "deserialize_finite_coordinate")]
        nx: f32,
        #[serde(deserialize_with = "deserialize_finite_coordinate")]
        ny: f32,
    },
    Wheel {
        #[serde(deserialize_with = "deserialize_wheel_delta")]
        delta_hi_res: i32,
    },
    Scroll {
        #[serde(deserialize_with = "deserialize_scroll_delta")]
        delta_x_q16_16: i64,
        #[serde(deserialize_with = "deserialize_scroll_delta")]
        delta_y_q16_16: i64,
        unit: PointerScrollUnitWire,
        #[serde(default)]
        phase: PointerScrollPhaseWire,
        #[serde(default)]
        momentum_phase: PointerScrollPhaseWire,
    },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum InputButtonStateWire {
    Pressed,
    Released,
    Repeated,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PointerScrollUnitWire {
    Line120,
    Pixels,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PointerScrollPhaseWire {
    #[default]
    None,
    MayBegin,
    Began,
    Changed,
    Stationary,
    Ended,
    Cancelled,
}

impl BrowserInputEdgeWire {
    pub(super) fn into_edge(self) -> hypercolor_core::input::BrowserInputEdge {
        use hypercolor_core::input::BrowserInputEdge;
        use hypercolor_types::event::{InputButtonState, PointerScrollPhase, PointerScrollUnit};

        let map_state = |state: InputButtonStateWire| match state {
            InputButtonStateWire::Pressed => InputButtonState::Pressed,
            InputButtonStateWire::Released => InputButtonState::Released,
            InputButtonStateWire::Repeated => InputButtonState::Repeated,
        };
        let map_unit = |unit: PointerScrollUnitWire| match unit {
            PointerScrollUnitWire::Line120 => PointerScrollUnit::Line120,
            PointerScrollUnitWire::Pixels => PointerScrollUnit::Pixels,
        };
        let map_phase = |phase: PointerScrollPhaseWire| match phase {
            PointerScrollPhaseWire::None => PointerScrollPhase::None,
            PointerScrollPhaseWire::MayBegin => PointerScrollPhase::MayBegin,
            PointerScrollPhaseWire::Began => PointerScrollPhase::Began,
            PointerScrollPhaseWire::Changed => PointerScrollPhase::Changed,
            PointerScrollPhaseWire::Stationary => PointerScrollPhase::Stationary,
            PointerScrollPhaseWire::Ended => PointerScrollPhase::Ended,
            PointerScrollPhaseWire::Cancelled => PointerScrollPhase::Cancelled,
        };

        match self {
            Self::Key { key, state } => BrowserInputEdge::Key {
                key,
                state: map_state(state),
            },
            Self::Button { button, state } => BrowserInputEdge::Button {
                button,
                state: map_state(state),
            },
            Self::Move { nx, ny } => BrowserInputEdge::Move {
                norm_x: nx,
                norm_y: ny,
            },
            Self::Wheel { delta_hi_res } => BrowserInputEdge::Wheel { delta_hi_res },
            Self::Scroll {
                delta_x_q16_16,
                delta_y_q16_16,
                unit,
                phase,
                momentum_phase,
            } => BrowserInputEdge::Scroll {
                delta_x_q16_16,
                delta_y_q16_16,
                unit: map_unit(unit),
                phase: map_phase(phase),
                momentum_phase: map_phase(momentum_phase),
            },
        }
    }
}

pub(super) fn validate_interactive_preview_id(preview_id: &str) -> Result<(), WsProtocolError> {
    if preview_id.is_empty() {
        return Err(WsProtocolError::invalid_request(
            "preview_id cannot be empty",
        ));
    }
    if preview_id.len() > INTERACTIVE_PREVIEW_ID_MAX_BYTES {
        return Err(WsProtocolError::invalid_request(format!(
            "preview_id cannot exceed {INTERACTIVE_PREVIEW_ID_MAX_BYTES} UTF-8 bytes"
        )));
    }
    if preview_id.chars().any(char::is_control) {
        return Err(WsProtocolError::invalid_request(
            "preview_id cannot contain control characters",
        ));
    }
    Ok(())
}

fn deserialize_interactive_preview_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let preview_id = String::deserialize(deserializer)?;
    validate_interactive_preview_id(&preview_id)
        .map_err(|error| serde::de::Error::custom(error.message))?;
    Ok(preview_id)
}

pub(super) fn validate_interactive_preview_shape(
    width: u32,
    height: u32,
    format: CanvasFormat,
) -> Result<(), WsProtocolError> {
    validate_preview_surface_resource(width, height, format)
        .map(|_| ())
        .map_err(WsProtocolError::invalid_request)
}

pub(super) fn validate_preview_surface_resource(
    width: u32,
    height: u32,
    format: CanvasFormat,
) -> Result<usize, String> {
    if format == CanvasFormat::Jpeg && (width > u32::from(u16::MAX) || height > u32::from(u16::MAX))
    {
        return Err(format!(
            "JPEG preview axes cannot exceed {} pixels; requested {width}x{height}",
            u16::MAX
        ));
    }
    validate_preview_surface_bytes(width, height)
}

pub(super) fn validate_preview_surface_bytes(width: u32, height: u32) -> Result<usize, String> {
    let byte_len = SurfaceDescriptor::rgba8888(width, height)
        .try_non_empty_byte_len()
        .map_err(|error| error.to_string())?;
    if byte_len > MAX_PREVIEW_PUBLICATION_BYTES {
        return Err(format!(
            "preview surface {width}x{height} requires {byte_len} decoded bytes, exceeding the \
             {MAX_PREVIEW_PUBLICATION_BYTES}-byte publication budget"
        ));
    }
    Ok(byte_len)
}

fn deserialize_input_edges<'de, D>(deserializer: D) -> Result<Vec<BrowserInputEdgeWire>, D::Error>
where
    D: Deserializer<'de>,
{
    struct InputEdgeBatchVisitor;

    impl<'de> Visitor<'de> for InputEdgeBatchVisitor {
        type Value = Vec<BrowserInputEdgeWire>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                formatter,
                "at most {MAX_INPUT_INJECT_EVENTS} browser input edges"
            )
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            if sequence
                .size_hint()
                .is_some_and(|count| count > MAX_INPUT_INJECT_EVENTS)
            {
                return Err(de::Error::invalid_length(
                    sequence.size_hint().unwrap_or_default(),
                    &self,
                ));
            }

            let initial_capacity = sequence
                .size_hint()
                .unwrap_or_default()
                .min(MAX_INPUT_INJECT_EVENTS);
            let mut edges = Vec::with_capacity(initial_capacity);
            loop {
                if edges.len() == MAX_INPUT_INJECT_EVENTS {
                    return match sequence.next_element::<IgnoredAny>()? {
                        Some(_) => Err(de::Error::invalid_length(
                            MAX_INPUT_INJECT_EVENTS.saturating_add(1),
                            &self,
                        )),
                        None => Ok(edges),
                    };
                }
                match sequence.next_element()? {
                    Some(edge) => edges.push(edge),
                    None => return Ok(edges),
                }
            }
        }
    }

    deserializer.deserialize_seq(InputEdgeBatchVisitor)
}

fn deserialize_input_name<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    struct InputNameVisitor;

    impl Visitor<'_> for InputNameVisitor {
        type Value = String;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                formatter,
                "a non-empty browser input name no longer than {MAX_INPUT_NAME_BYTES} bytes"
            )
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            validate_input_name(value, &self)?;
            Ok(value.to_owned())
        }

        fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            validate_input_name(&value, &self)?;
            Ok(value)
        }
    }

    deserializer.deserialize_string(InputNameVisitor)
}

fn validate_input_name<E>(value: &str, expected: &dyn de::Expected) -> Result<(), E>
where
    E: de::Error,
{
    if value.is_empty() || value.len() > MAX_INPUT_NAME_BYTES || value.chars().any(char::is_control)
    {
        return Err(E::invalid_length(value.len(), expected));
    }
    Ok(())
}

fn deserialize_input_button<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = deserialize_input_name(deserializer)?;
    if matches!(value.as_str(), "left" | "right" | "middle") {
        Ok(value)
    } else {
        Err(de::Error::unknown_variant(
            &value,
            &["left", "right", "middle"],
        ))
    }
}

fn deserialize_wheel_delta<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    let value = i32::deserialize(deserializer)?;
    if value.unsigned_abs() <= MAX_INPUT_WHEEL_DELTA.unsigned_abs() {
        Ok(value)
    } else {
        Err(de::Error::custom(format_args!(
            "browser input wheel delta must be within ±{MAX_INPUT_WHEEL_DELTA}"
        )))
    }
}

fn deserialize_scroll_delta<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = i64::deserialize(deserializer)?;
    if value
        .checked_abs()
        .is_some_and(|magnitude| magnitude <= MAX_INPUT_SCROLL_Q16_16)
    {
        Ok(value)
    } else {
        Err(de::Error::custom(format_args!(
            "browser input scroll delta must be within ±{MAX_INPUT_SCROLL_Q16_16}"
        )))
    }
}

pub(super) fn deserialize_finite_coordinate<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: Deserializer<'de>,
{
    let value = f32::deserialize(deserializer)?;
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err(de::Error::custom(
            "browser input coordinates must be finite and normalized to [0, 1]",
        ))
    }
}

macro_rules! define_server_messages {
    ($($(#[$meta:meta])* $variant:ident $body:tt),+ $(,)?) => {
        /// Server-to-client acknowledgment messages.
        #[derive(Debug, Serialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        pub(super) enum ServerMessage {
            $($(#[$meta])* $variant $body),+
        }

        pub(super) fn server_message_vocabulary() -> Vec<String> {
            vec![$(hypercolor_types::event::pascal_to_snake_case(stringify!($variant))),+]
        }
    };
}

define_server_messages! {
    /// Initial hello with state snapshot.
    Hello {
        version: String,
        server: ServerIdentity,
        state: HelloState,
        capabilities: Vec<String>,
        subscriptions: Vec<ActiveSubscription>,
    },
    /// Subscribe acknowledgment: the connection's whole live subscription
    /// set, so a client always learns the state it ended up in rather
    /// than only the delta it asked for.
    Subscribed { topics: Vec<ActiveSubscription> },
    /// Unsubscribe acknowledgment, carrying what remains.
    Unsubscribed { topics: Vec<ActiveSubscription> },
    /// Addressed input injection acknowledgment.
    InputInjected {
        preview_id: String,
        accepted_events: usize,
    },
    /// Authoritative browser-input claim acknowledgment.
    InteractivePreviewAuthoritativeClaimed {
        preview_id: String,
        already_owned: bool,
    },
    /// Authoritative browser-input release acknowledgment.
    InteractivePreviewAuthoritativeReleased { preview_id: String, released: bool },
    /// Event relay from the bus.
    Event {
        event: String,
        timestamp: String,
        data: serde_json::Value,
    },
    /// Periodic performance metrics snapshot.
    Metrics {
        timestamp: String,
        data: MetricsPayload,
    },
    /// Periodic per-device output telemetry snapshot.
    DeviceMetrics {
        timestamp: String,
        data: DeviceMetricsSnapshot,
    },
    /// Latest host sensor snapshot.
    Sensors {
        timestamp: String,
        data: SystemSnapshot,
    },
    /// Backpressure warning for dropped binary payloads on one topic.
    Backpressure {
        dropped_frames: u32,
        topic: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        recommendation: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        suggested_fps: Option<f64>,
    },
    /// Protocol-level request error.
    Error {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
    },
    /// Command response envelope for WS command execution.
    Response {
        id: String,
        status: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<serde_json::Value>,
    },
}

pub(super) fn json_payload_manifest() -> serde_json::Value {
    serde_json::json!({
        "timed_input_event_v1": {
            "schema_version": 1,
            "event": "input_event_received",
            "required_fields": ["event"],
            "optional_fields": {
                "at_ms": 0,
                "seq": 0,
                "physical_code": null,
                "repeat_count": 1
            },
            "description": "Canonical captured input edge. Missing timing and metadata fields decode with their listed defaults for compatibility with the prior event-only payload.",
            "topic": "input_events"
        },
        "input_source_status_changed_v1": {
            "schema_version": 1,
            "event": "input_source_status_changed",
            "required_fields": [
                "source_id",
                "kind",
                "backend",
                "configured",
                "consented",
                "demanded",
                "active_consumer_count",
                "state",
                "freshness",
                "source_graph_generation",
                "session_generation",
                "resource_count",
                "denied_resource_count",
                "retired"
            ],
            "optional_fields": {
                "lifecycle_issue_code": null,
                "freshness_issue_code": null
            },
            "description": "Coalesced input-source lifecycle and freshness transition. Contains operational metadata only and never captured input contents.",
            "topic": "events"
        },
        "macos_daemon_ownership_changed_v1": {
            "schema_version": 1,
            "topic": "events",
            "event": "macos_daemon_ownership_changed",
            "required_fields": ["active_owner", "owner_epoch"],
            "optional_fields": {
                "conflict": null,
                "recovery_required": null
            },
            "description": "Authoritative macOS daemon topology snapshot. The event reports ownership state only and cannot request an owner change."
        }
    })
}

/// The handshake snapshot.
///
/// Deliberately says nothing about what is rendering: the live tree is
/// multi-zone and multi-layer, so a single `effect` name could only
/// ever describe one corner of it. Clients read `/scene` for content
/// and follow the events channel for changes (Spec 78 §7.1).
#[derive(Debug, Serialize)]
pub(super) struct HelloState {
    pub(super) running: bool,
    pub(super) paused: bool,
    pub(super) brightness: u8,
    pub(super) fps: HelloFps,
    pub(super) scene: Option<SceneRef>,
    pub(super) layout: Option<NameRef>,
    pub(super) device_count: usize,
    pub(super) total_leds: usize,
}

#[derive(Debug, Serialize)]
pub(super) struct HelloFps {
    pub(super) target: u32,
    pub(super) capacity: f64,
    pub(super) delivered: f64,
}

#[derive(Debug, Serialize)]
pub(super) struct NameRef {
    pub(super) id: String,
    pub(super) name: String,
}

#[derive(Debug, Serialize)]
pub(super) struct SceneRef {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) snapshot_locked: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct MetricsPayload {
    pub(super) fps: MetricsFps,
    pub(super) frame_time: MetricsFrameTime,
    pub(super) input_latency: MetricsSessionLatency,
    pub(super) stages: MetricsStages,
    pub(super) pacing: MetricsPacing,
    pub(super) effect_health: MetricsEffectHealth,
    pub(super) timeline: MetricsTimeline,
    pub(super) render_surfaces: MetricsRenderSurfaces,
    pub(super) preview: MetricsPreview,
    pub(super) display_output: MetricsDisplayOutput,
    pub(super) copies: MetricsCopies,
    pub(super) memory: MetricsMemory,
    pub(super) devices: MetricsDevices,
    pub(super) websocket: MetricsWebsocket,
}

#[derive(Debug, Serialize)]
pub(super) struct MetricsFps {
    pub(super) target: u32,
    pub(super) ceiling: u32,
    pub(super) capacity: f64,
    pub(super) delivered: f64,
    pub(super) dropped: u32,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_field_names,
    reason = "JSON keys mirror protocol field names from the WebSocket spec"
)]
pub(super) struct MetricsFrameTime {
    pub(super) avg_ms: f64,
    pub(super) p95_ms: f64,
    pub(super) p99_ms: f64,
    pub(super) max_ms: f64,
}

#[derive(Debug, Serialize)]
pub(super) struct MetricsSessionLatency {
    pub(super) sample_count: u64,
    pub(super) avg_ms: f64,
    pub(super) p95_ms: f64,
    pub(super) p99_ms: f64,
    pub(super) max_ms: f64,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_field_names,
    reason = "JSON keys mirror protocol field names from the WebSocket spec"
)]
pub(super) struct MetricsStages {
    pub(super) input_sampling_ms: f64,
    pub(super) producer_rendering_ms: f64,
    pub(super) producer_effect_rendering_ms: f64,
    #[serde(rename = "producer_preview_compose_ms")]
    pub(super) producer_scene_compose_ms: f64,
    pub(super) composition_ms: f64,
    pub(super) effect_rendering_ms: f64,
    pub(super) spatial_sampling_ms: f64,
    pub(super) device_output_ms: f64,
    pub(super) preview_postprocess_ms: f64,
    pub(super) event_bus_ms: f64,
    pub(super) publish_frame_data_ms: f64,
    pub(super) publish_group_canvas_ms: f64,
    pub(super) publish_preview_ms: f64,
    pub(super) publish_events_ms: f64,
    pub(super) coordination_overhead_ms: f64,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_field_names,
    reason = "JSON keys mirror protocol field names from the WebSocket spec"
)]
pub(super) struct MetricsPacing {
    pub(super) jitter_avg_ms: f64,
    pub(super) jitter_p95_ms: f64,
    pub(super) jitter_max_ms: f64,
    pub(super) wake_delay_avg_ms: f64,
    pub(super) wake_delay_p95_ms: f64,
    pub(super) wake_delay_max_ms: f64,
    pub(super) push_avg_ms: f64,
    pub(super) push_p95_ms: f64,
    pub(super) push_max_ms: f64,
    pub(super) publish_avg_ms: f64,
    pub(super) publish_p95_ms: f64,
    pub(super) publish_max_ms: f64,
    pub(super) frame_age_ms: f64,
    pub(super) reused_inputs: u32,
    pub(super) reused_canvas: u32,
    pub(super) retained_effect: u32,
    pub(super) retained_screen: u32,
    pub(super) composition_bypassed: u32,
    pub(super) gpu_zone_sampling: u32,
    pub(super) gpu_sample_deferred: u32,
    pub(super) gpu_sample_stale: u32,
    pub(super) gpu_sample_retry_hit: u32,
    pub(super) gpu_sample_queue_saturated: u32,
    pub(super) gpu_sample_wait_blocked: u32,
    pub(super) gpu_sample_cpu_fallback: u32,
    pub(super) preview_surface: u32,
    pub(super) scene_canvas_forced_surface: u32,
    pub(super) gpu_readback_failed_frames: u32,
    pub(super) output_error_frames: u32,
    pub(super) full_frame_copy_frames: u32,
    pub(super) output_current_frame: u32,
    pub(super) output_published_frame: u32,
    pub(super) output_routed_reuse: u32,
    pub(super) output_reused_published_frame: u32,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_field_names,
    reason = "JSON keys mirror protocol field names from the WebSocket spec"
)]
pub(super) struct MetricsEffectHealth {
    pub(super) errors_total: u64,
    pub(super) fallbacks_applied_total: u64,
    pub(super) producer_gpu_readback_failures_total: u64,
    pub(super) servo_soft_stalls_total: u64,
    pub(super) servo_breaker_opens_total: u64,
    pub(super) servo_session_creates_total: u64,
    pub(super) servo_session_create_failures_total: u64,
    pub(super) servo_session_create_wait_total_ms: f64,
    pub(super) servo_session_create_wait_max_ms: f64,
    pub(super) servo_page_loads_total: u64,
    pub(super) servo_page_load_failures_total: u64,
    pub(super) servo_page_load_wait_total_ms: f64,
    pub(super) servo_page_load_wait_max_ms: f64,
    pub(super) servo_renderer_loads_total: u64,
    pub(super) servo_renderer_load_failures_total: u64,
    pub(super) servo_renderer_load_wait_total_ms: f64,
    pub(super) servo_renderer_load_wait_max_ms: f64,
    pub(super) servo_detached_destroys_total: u64,
    pub(super) servo_detached_destroy_failures_total: u64,
    pub(super) servo_destroy_wait_total_ms: f64,
    pub(super) servo_destroy_wait_max_ms: f64,
    pub(super) servo_render_requests_total: u64,
    pub(super) servo_render_queue_wait_total_ms: f64,
    pub(super) servo_render_queue_wait_max_ms: f64,
    pub(super) servo_render_scene_requests_total: u64,
    pub(super) servo_render_scene_queue_wait_total_ms: f64,
    pub(super) servo_render_scene_queue_wait_max_ms: f64,
    pub(super) servo_render_display_requests_total: u64,
    pub(super) servo_render_display_queue_wait_total_ms: f64,
    pub(super) servo_render_display_queue_wait_max_ms: f64,
    pub(super) servo_render_queue_depth: u64,
    pub(super) servo_render_queue_depth_max: u64,
    pub(super) servo_render_superseded_total: u64,
    pub(super) servo_render_pending_age_max_ms: f64,
    pub(super) servo_render_cpu_frames_total: u64,
    pub(super) servo_render_cached_frames_total: u64,
    pub(super) servo_render_gpu_frames_total: u64,
    pub(super) servo_gpu_import_failures_total: u64,
    pub(super) servo_gpu_import_fallbacks_total: u64,
    pub(super) servo_gpu_import_fallback_reason: Option<&'static str>,
    pub(super) servo_gpu_import_windows_sync_mode: Option<&'static str>,
    pub(super) servo_gpu_import_stale_frame_total: u64,
    pub(super) servo_gpu_import_adapter_mismatch_total: u64,
    pub(super) servo_gpu_import_slot_count: u64,
    pub(super) servo_gpu_import_pending_slots: u64,
    pub(super) servo_gpu_import_pending_slots_max: u64,
    pub(super) servo_gpu_import_completed_slots: u64,
    pub(super) servo_gpu_import_available_slots: u64,
    pub(super) servo_gpu_import_available_slots_min: u64,
    pub(super) servo_gpu_import_oldest_pending_age_max_ms: f64,
    pub(super) servo_gpu_import_blit_total_ms: f64,
    pub(super) servo_gpu_import_blit_max_ms: f64,
    pub(super) servo_gpu_import_sync_total_ms: f64,
    pub(super) servo_gpu_import_sync_max_ms: f64,
    pub(super) servo_gpu_import_total_ms: f64,
    pub(super) servo_gpu_import_max_ms: f64,
    pub(super) producer_cpu_frames_total: u64,
    pub(super) producer_gpu_frames_total: u64,
    pub(super) producer_gpu_cpu_materialization_blocked_total: u64,
    pub(super) sparkleflinger_gpu_source_upload_skipped_total: u64,
    pub(super) sparkleflinger_media_texture_allocations_total: u64,
    pub(super) sparkleflinger_media_texture_upload_bytes_total: u64,
    pub(super) sparkleflinger_display_finalize_rgba_attempts_total: u64,
    pub(super) sparkleflinger_display_finalize_yuv_attempts_total: u64,
    pub(super) sparkleflinger_display_finalize_successes_total: u64,
    pub(super) sparkleflinger_display_finalize_misses_total: u64,
    pub(super) sparkleflinger_display_finalize_latches_total: u64,
    pub(super) sparkleflinger_display_finalize_blocking_wait_total_ms: f64,
    pub(super) sparkleflinger_display_finalize_blocking_wait_max_ms: f64,
    pub(super) sparkleflinger_display_finalize_surface_reallocs_total: u64,
    pub(super) servo_render_evaluate_scripts_total_ms: f64,
    pub(super) servo_render_evaluate_scripts_max_ms: f64,
    pub(super) servo_render_event_loop_total_ms: f64,
    pub(super) servo_render_event_loop_max_ms: f64,
    pub(super) servo_render_paint_total_ms: f64,
    pub(super) servo_render_paint_max_ms: f64,
    pub(super) servo_render_readback_total_ms: f64,
    pub(super) servo_render_readback_max_ms: f64,
    pub(super) servo_render_frame_total_ms: f64,
    pub(super) servo_render_frame_max_ms: f64,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_field_names,
    reason = "JSON keys mirror protocol field names from the WebSocket spec"
)]
pub(super) struct MetricsTimeline {
    pub(super) frame_token: u64,
    pub(super) compositor_backend: String,
    pub(super) output_frame_source: String,
    pub(super) output_reuses_published_frame: bool,
    pub(super) output_brightness_bits: u32,
    pub(super) output_brightness_generation: u64,
    pub(super) output_routing_signature: u64,
    pub(super) output_zone_shape_signature: u64,
    pub(super) output_unassigned_behavior_generation: u64,
    pub(super) devices_written: u32,
    pub(super) total_leds: u32,
    pub(super) gpu_zone_sampling: bool,
    pub(super) gpu_sample_deferred: bool,
    pub(super) gpu_sample_stale: bool,
    pub(super) gpu_sample_retry_hit: bool,
    pub(super) gpu_sample_queue_saturated: bool,
    pub(super) gpu_sample_wait_blocked: bool,
    pub(super) gpu_sample_cpu_fallback: bool,
    pub(super) preview_surface: bool,
    pub(super) scene_canvas_forced_surface: bool,
    pub(super) cpu_readback_skipped: bool,
    pub(super) gpu_readback_failed: bool,
    pub(super) budget_ms: f64,
    pub(super) wake_late_ms: f64,
    pub(super) logical_layer_count: u32,
    pub(super) render_group_count: u32,
    pub(super) scene_active: bool,
    pub(super) scene_transition_active: bool,
    pub(super) scene_snapshot_done_ms: f64,
    pub(super) input_done_ms: f64,
    /// Duration, not a milestone: finalizing the *previous* frame's deferred
    /// GPU zone readback, which runs after input sampling and before
    /// composition starts. `producer_done_ms` and `composition_done_ms` are
    /// derived from the composition stage's own clock while
    /// `sampling_done_ms` is absolute, so this time falls inside the
    /// `sampling_done_ms - composition_done_ms` difference. A consumer
    /// charting phases has to subtract it there.
    pub(super) deferred_sample_ms: f64,
    pub(super) producer_done_ms: f64,
    pub(super) composition_done_ms: f64,
    /// Duration, not a milestone: submitting and resolving the GPU preview
    /// surface, non-zero only while a preview consumer is attached. It runs
    /// between composition and sampling, so it lands in the same
    /// `sampling_done_ms - composition_done_ms` difference as
    /// `deferred_sample_ms` and needs the same subtraction.
    pub(super) preview_advance_ms: f64,
    pub(super) sampling_done_ms: f64,
    pub(super) output_done_ms: f64,
    pub(super) publish_done_ms: f64,
    pub(super) frame_done_ms: f64,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_field_names,
    reason = "JSON keys mirror protocol field names from the WebSocket spec"
)]
pub(super) struct MetricsCopies {
    pub(super) full_frame_count: u32,
    pub(super) full_frame_kb: f64,
    pub(super) producer_full_frame_count: u32,
    pub(super) producer_full_frame_kb: f64,
    pub(super) producer_reason: Option<&'static str>,
    pub(super) publication_full_frame_count: u32,
    pub(super) publication_full_frame_kb: f64,
    pub(super) publication_reason: Option<&'static str>,
    pub(super) session_full_frame_count: u64,
    pub(super) session_full_frame_frames: u64,
    pub(super) session_full_frame_bytes: u64,
}

#[derive(Debug, Serialize)]
pub(super) struct MetricsRenderSurfaces {
    pub(super) canvas_receivers: u32,
    /// Monotonic counter: how many times the render-group scene surface pool
    /// hit its growth cap and had to reuse a still-shared slot, forcing
    /// a fresh `Canvas::new` on every frame. A rising value means the
    /// cap is too low for current fan-out.
    pub(super) scene_pool_saturation_reallocs: u64,
    /// Same counter summed across per-group direct-canvas pools.
    pub(super) direct_pool_saturation_reallocs: u64,
    /// Current slot count above the scene surface pool's initial size.
    /// Benign when stable — the pool converged on its working set. A
    /// climbing value over time could indicate a pinned-Arc leak.
    pub(super) scene_pool_grown_slots: u32,
    /// Same gauge summed across per-group direct-canvas pools.
    pub(super) direct_pool_grown_slots: u32,
    pub(super) scene_pool_slot_count: u32,
    pub(super) scene_pool_max_slots: u32,
    pub(super) direct_pool_slot_count: u32,
    pub(super) direct_pool_max_slots: u32,
    pub(super) scene_pool_shared_published_slots: u32,
    pub(super) scene_pool_max_ref_count: u32,
    pub(super) direct_pool_shared_published_slots: u32,
    pub(super) direct_pool_max_ref_count: u32,
    pub(super) scene_pool_free_slots: u32,
    pub(super) scene_pool_published_slots: u32,
    pub(super) scene_pool_dequeued_slots: u32,
    pub(super) direct_pool_free_slots: u32,
    pub(super) direct_pool_published_slots: u32,
    pub(super) direct_pool_dequeued_slots: u32,
    pub(super) preview_pool_slot_count: u32,
    pub(super) preview_pool_free_slots: u32,
    pub(super) preview_pool_published_slots: u32,
    pub(super) preview_pool_dequeued_slots: u32,
    pub(super) compositor_pool_slot_count: u32,
    pub(super) compositor_pool_free_slots: u32,
    pub(super) compositor_pool_published_slots: u32,
    pub(super) compositor_pool_dequeued_slots: u32,
}

#[derive(Debug, Serialize)]
pub(super) struct MetricsPreview {
    pub(super) canvas_receivers: u32,
    pub(super) scene_canvas_receivers: u32,
    pub(super) screen_canvas_receivers: u32,
    pub(super) web_viewport_canvas_receivers: u32,
    pub(super) zone_preview_receivers: u32,
    pub(super) canvas_frames_published: u64,
    pub(super) scene_canvas_frames_published: u64,
    pub(super) screen_canvas_frames_published: u64,
    pub(super) web_viewport_canvas_frames_published: u64,
    pub(super) zone_preview_frames_published: u64,
    pub(super) latest_canvas_frame_number: u32,
    pub(super) latest_scene_canvas_frame_number: u32,
    pub(super) latest_screen_canvas_frame_number: u32,
    pub(super) latest_web_viewport_canvas_frame_number: u32,
    pub(super) latest_zone_preview_frame_number: u32,
    pub(super) canvas_demand: MetricsPreviewDemand,
    pub(super) scene_canvas_demand: MetricsPreviewDemand,
    pub(super) screen_canvas_demand: MetricsPreviewDemand,
    pub(super) web_viewport_canvas_demand: MetricsPreviewDemand,
    pub(super) zone_preview_demand: MetricsPreviewDemand,
}

#[derive(Debug, Serialize)]
pub(super) struct MetricsDisplayOutput {
    pub(super) captured_devices: usize,
    pub(super) preview_subscribers: usize,
    pub(super) write_attempts_total: u64,
    pub(super) write_successes_total: u64,
    pub(super) write_failures_total: u64,
    pub(super) retry_attempts_total: u64,
    pub(super) display_lane: MetricsDisplayLane,
    pub(super) last_failure_age_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
#[allow(
    clippy::struct_field_names,
    reason = "JSON keys mirror protocol field names from the WebSocket spec"
)]
pub(super) struct MetricsDisplayLane {
    pub(super) display_frames_total: u64,
    pub(super) display_frames_delayed_for_led_total: u64,
    pub(super) display_led_priority_wait_total_ms: f64,
    pub(super) display_led_priority_wait_max_ms: f64,
}

#[derive(Debug, Serialize)]
pub(super) struct MetricsPreviewDemand {
    pub(super) subscribers: u32,
    pub(super) max_fps: u32,
    pub(super) max_width: u32,
    pub(super) max_height: u32,
    pub(super) any_full_resolution: bool,
    pub(super) any_rgb: bool,
    pub(super) any_rgba: bool,
    pub(super) any_jpeg: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct MetricsMemory {
    pub(super) daemon_rss_mb: f64,
    pub(super) servo_rss_mb: f64,
    pub(super) canvas_buffer_kb: u32,
}

#[derive(Debug, Serialize)]
pub(super) struct MetricsDevices {
    pub(super) connected: usize,
    pub(super) total_leds: usize,
    pub(super) output_errors: u32,
}

#[derive(Debug, Serialize)]
pub(super) struct MetricsWebsocket {
    pub(super) client_count: usize,
    pub(super) bytes_sent_per_sec: f64,
    pub(super) frame_payload_builds: u64,
    pub(super) frame_payload_cache_hits: u64,
    pub(super) canvas_payload_builds: u64,
    pub(super) canvas_payload_cache_hits: u64,
    pub(super) preview_publications_queued: u64,
    pub(super) preview_publications_replaced: u64,
    pub(super) preview_publications_evicted: u64,
    pub(super) preview_publications_rejected: u64,
    pub(super) preview_publications_sent: u64,
    pub(super) preview_chunks_sent: u64,
    pub(super) preview_queue_bytes: usize,
}

/// A WS command refusal, rendered with the same code vocabulary REST
/// serves (Spec 78 §7.1).
///
/// The parallel WS-only code set is deleted: a client that already
/// knows `malformed_request`, `validation_error`, and `forbidden` from
/// the REST envelope reads a socket refusal without a second table.
#[derive(Debug)]
pub(super) struct WsProtocolError {
    pub(super) code: &'static str,
    pub(super) message: String,
    pub(super) details: Option<serde_json::Value>,
}

impl WsProtocolError {
    pub(super) fn invalid_request(message: impl Into<String>) -> Self {
        DomainError::malformed(message).into()
    }

    pub(super) fn forbidden(message: impl Into<String>, details: serde_json::Value) -> Self {
        DomainError::forbidden_details(message, details).into()
    }

    pub(super) fn invalid_config(field: impl Into<String>, reason: impl Into<String>) -> Self {
        let field = field.into();
        let reason = reason.into();
        DomainError::validation_details(
            format!("Invalid configuration for {field}: {reason}"),
            json!({"field": field, "reason": reason}),
        )
        .into()
    }

    pub(super) fn invalid_config_resource(
        field: impl Into<String>,
        width: u32,
        height: u32,
        reason: String,
    ) -> Self {
        let field = field.into();
        DomainError::validation_details(
            format!("Invalid configuration for {field}: {reason}"),
            json!({
                "field": field,
                "reason": reason,
                "width": width,
                "height": height,
                "max_publication_bytes": MAX_PREVIEW_PUBLICATION_BYTES,
            }),
        )
        .into()
    }

    pub(super) fn into_message(self) -> ServerMessage {
        ServerMessage::Error {
            code: self.code.to_owned(),
            message: self.message,
            details: self.details,
        }
    }
}

impl From<DomainError> for WsProtocolError {
    fn from(error: DomainError) -> Self {
        let code = error.code();
        let detail = error.detail();
        Self {
            code,
            message: detail.message,
            details: detail.details,
        }
    }
}

pub(super) fn frame_selection_hash(selected: &[String]) -> u64 {
    if selected.iter().any(|zone| zone == "all") {
        return 0;
    }

    let mut hasher = DefaultHasher::new();
    selected.len().hash(&mut hasher);
    for zone in selected {
        zone.hash(&mut hasher);
    }
    hasher.finish()
}

/// Validate one wire selector into a topic plus its canonical key.
fn parse_selector(topic: &str, key: Option<&str>) -> Result<TopicSelection, WsProtocolError> {
    let parsed = TopicId::parse(topic)
        .ok_or_else(|| WsProtocolError::invalid_request(format!("Unknown topic '{topic}'")))?;
    // The key the topic's own key type accepts, canonicalized — the
    // table stores what the boundary validated, never raw client text.
    let key = (parsed.vtable().validate_key)(key).map_err(|error| {
        WsProtocolError::invalid_request(format!("Invalid key for topic '{topic}': {error}"))
    })?;
    Ok(TopicSelection { topic: parsed, key })
}

/// Parse a subscribe message's `topics` array into validated requests.
pub(super) fn parse_subscriptions(
    entries: &[TopicSubscription],
) -> Result<Vec<SubscriptionRequest>, WsProtocolError> {
    if entries.is_empty() {
        return Err(WsProtocolError::invalid_request(
            "topics must contain at least one subscription",
        ));
    }

    let mut parsed: Vec<SubscriptionRequest> = Vec::with_capacity(entries.len());
    for entry in entries {
        let selection = parse_selector(&entry.topic, entry.key.as_deref())?;
        // Two entries for one subscription means the client does not
        // agree with itself about which config wins; resolving that
        // silently would hide it from the only party who can fix it.
        if parsed
            .iter()
            .any(|existing| existing.selection == selection)
        {
            return Err(WsProtocolError::invalid_request(format!(
                "Duplicate subscription for topic '{}'",
                entry.topic
            )));
        }
        parsed.push(SubscriptionRequest {
            selection,
            config: entry.config.clone(),
        });
    }

    Ok(parsed)
}

/// Parse an unsubscribe message's `topics` array into validated selectors.
pub(super) fn parse_selectors(
    selectors: &[TopicSelector],
) -> Result<Vec<TopicSelection>, WsProtocolError> {
    if selectors.is_empty() {
        return Err(WsProtocolError::invalid_request(
            "topics must contain at least one subscription",
        ));
    }

    selectors
        .iter()
        .map(|selector| parse_selector(&selector.topic, selector.key.as_deref()))
        .collect()
}

pub(crate) fn ws_capabilities() -> Vec<String> {
    let mut capabilities: Vec<String> = TopicId::ALL
        .iter()
        .map(|topic| topic.as_str().to_owned())
        .collect();
    capabilities.push("commands".to_owned());
    capabilities.push("canvas_format_jpeg".to_owned());
    capabilities.push("interactive_previews".to_owned());
    capabilities.push("wide_preview_frames".to_owned());
    capabilities.push("preview_chunking".to_owned());
    capabilities
}

pub(super) fn event_message_parts(
    event: &hypercolor_types::event::HypercolorEvent,
) -> (String, serde_json::Value) {
    if let hypercolor_types::event::HypercolorEvent::ExtensionStateChanged {
        source,
        kind,
        payload,
    } = event
        && source == hypercolor_core::bus::INPUT_STATUS_EVENT_SOURCE
        && kind == hypercolor_core::bus::INPUT_STATUS_EVENT_KIND
    {
        return ("input_source_status_changed".to_owned(), payload.clone());
    }

    let serialized = serde_json::to_value(event).ok();
    let event_type = serialized
        .as_ref()
        .and_then(|value| value.get("type"))
        .and_then(serde_json::Value::as_str);

    let event_name = if let Some(event_type) = event_type {
        hypercolor_types::event::pascal_to_snake_case(event_type)
    } else {
        format!("{:?}", event.category()).to_lowercase()
    };
    let event_data = serialized
        .and_then(|value| value.get("data").cloned())
        .unwrap_or_else(|| json!({}));

    (event_name, event_data)
}

pub(super) fn should_relay_event(
    event: &hypercolor_types::event::HypercolorEvent,
    topics: TopicSet,
) -> bool {
    if matches!(
        event,
        hypercolor_types::event::HypercolorEvent::FrameRendered { .. }
    ) {
        return topics.contains(TopicId::FrameEvents);
    }

    // Host input events carry keystroke data and never ride the default
    // events channel: they need the control-authorized input_events
    // subscription, mirroring the screen-capture channels.
    if matches!(
        event,
        hypercolor_types::event::HypercolorEvent::InputEventReceived { .. }
    ) {
        return topics.contains(TopicId::InputEvents);
    }

    topics.contains(TopicId::Events)
}
