//! The WebSocket topic registry (Spec 76 §5).
//!
//! One declarative macro turns a topic topology into everything the
//! wire needs to agree on: the topic id enum with its wire names, the
//! membership set, per-topic config defaults and transactional patch
//! application, key validation, control-tier gating, and — enforced at
//! COMPILE TIME — disjoint binary-tag ownership across topics. Adding
//! a topic is one macro entry plus a relay registration on the daemon
//! side; the eight hand-maintained places the old protocol required
//! collapse into this file's output.
//!
//! Layering: this module owns WIRE facts only (names, keys, configs,
//! patches, tags, gating). Runtime facts — which bus lane feeds a
//! topic, what engine demand a subscription implies — live in the
//! daemon's runtime table, keyed by [`TopicId`], because they name
//! engine types this crate must not know.
//!
//! # Patch semantics
//!
//! Patches are transactional by construction:
//! [`apply_patch_transactionally`] clones the current config, applies
//! every field, and returns the replacement only if the whole patch
//! validated — a failing field leaves the live config untouched. The
//! double-`Option` tri-state (`missing` = leave, `null` = clear,
//! `value` = set) stays expressible per field in each patch type.
//!
//! # Keys
//!
//! Unkeyed topics use `Key = ()`. Keyed topics carry their key on the
//! wire as a string beside the topic name; [`TopicKey::from_wire`]
//! validates at the boundary, so a keyed subscribe without a key (or
//! an unkeyed one with a key) is rejected before any state changes.

use std::collections::BTreeMap;

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Why a wire key was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// The topic takes no key but one was supplied.
    #[error("topic does not take a key")]
    UnexpectedKey,
    /// The topic requires a key and none was supplied.
    #[error("topic requires a key")]
    MissingKey,
    /// The supplied key failed the key type's validation.
    #[error("invalid key: {0}")]
    Invalid(String),
}

/// Why a patch was refused. The config it targeted is untouched.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field}: {reason}")]
pub struct PatchError {
    /// The offending patch field.
    pub field: &'static str,
    /// What it violated.
    pub reason: String,
}

impl PatchError {
    /// Build a patch rejection.
    #[must_use]
    pub fn new(field: &'static str, reason: impl Into<String>) -> Self {
        Self {
            field,
            reason: reason.into(),
        }
    }
}

/// A topic's subscription key. Unkeyed topics use `()`.
pub trait TopicKey: Sized + Clone + PartialEq {
    /// The wire form carried beside the topic name; `None` for
    /// unkeyed topics.
    fn to_wire(&self) -> Option<String>;
    /// Parse and validate the wire form.
    fn from_wire(key: Option<&str>) -> Result<Self, KeyError>;
}

impl TopicKey for () {
    fn to_wire(&self) -> Option<String> {
        None
    }

    fn from_wire(key: Option<&str>) -> Result<Self, KeyError> {
        match key {
            None => Ok(()),
            Some(_) => Err(KeyError::UnexpectedKey),
        }
    }
}

/// A topic's per-subscription configuration.
///
/// Blanket-implemented; `()` serves configless topics.
pub trait TopicConfig: Clone + PartialEq + Default + Serialize + DeserializeOwned {}

impl<T: Clone + PartialEq + Default + Serialize + DeserializeOwned> TopicConfig for T {}

/// A topic's config patch: every field validates before any lands.
pub trait TopicPatch<C>: DeserializeOwned {
    /// Apply onto `config`. Called on a CLONE by
    /// [`apply_patch_transactionally`]; implementations mutate freely
    /// and return the first failure.
    fn apply(&self, config: &mut C) -> Result<(), PatchError>;
}

/// The patch type for configless topics: deserializes only from
/// `null`/absent and refuses to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NoPatch;

impl<'de> serde::Deserialize<'de> for NoPatch {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.is_null() {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("topic accepts no config"))
        }
    }
}

impl TopicPatch<()> for NoPatch {
    fn apply(&self, _config: &mut ()) -> Result<(), PatchError> {
        Err(PatchError::new("config", "topic accepts no config"))
    }
}

/// One topic: the compile-time bundle of its wire facts.
pub trait WsTopic {
    /// The wire name (`"frames"`, `"display_preview"`, …).
    const NAME: &'static str;
    /// Binary tags this topic's codec owns. Disjointness across the
    /// registry is a compile-time assertion.
    const OWNED_TAGS: &'static [u8];
    /// Whether subscribing requires the control tier.
    const REQUIRES_CONTROL: bool;
    /// Subscription key type; `()` for unkeyed.
    type Key: TopicKey;
    /// Per-subscription config; `()` for configless.
    type Config: TopicConfig;
    /// Config patch; [`NoPatch`] for configless.
    type Patch: TopicPatch<Self::Config>;
}

/// One live subscription to a topic — invalid states (a keyed topic
/// without a key) are unrepresentable.
#[derive(Debug, Clone, PartialEq)]
pub struct Subscription<T: WsTopic> {
    /// The subscription's key.
    pub key: T::Key,
    /// The subscription's config.
    pub config: T::Config,
}

/// Apply `patch` onto a clone of `config`; the original is untouched
/// unless the WHOLE patch validates.
pub fn apply_patch_transactionally<C, P>(config: &C, patch: &P) -> Result<C, PatchError>
where
    C: Clone,
    P: TopicPatch<C>,
{
    let mut candidate = config.clone();
    patch.apply(&mut candidate)?;
    Ok(candidate)
}

/// Type-erased per-topic operations for wire-boundary dispatch — one
/// table replaces the hand-unrolled per-channel match chains.
pub struct TopicVTable {
    /// The topic's wire name.
    pub name: &'static str,
    /// Tags the topic's codec owns.
    pub owned_tags: &'static [u8],
    /// Control-tier gate.
    pub requires_control: bool,
    /// Whether the topic takes a key.
    pub keyed: bool,
    /// Whether the topic takes config (false = configless).
    pub configurable: bool,
    /// The default config as JSON (`null` for configless topics).
    pub default_config_json: fn() -> serde_json::Value,
    /// Validate a wire key for this topic without materializing it.
    pub validate_key: fn(Option<&str>) -> Result<(), KeyError>,
    /// Transactionally apply a JSON patch onto a JSON config,
    /// returning the replacement config. Errors leave the input
    /// semantics untouched by construction.
    pub apply_patch_json: fn(
        config: &serde_json::Value,
        patch: &serde_json::Value,
    ) -> Result<serde_json::Value, PatchError>,
}

/// Compile-time disjointness check over every topic's owned tags.
/// Two topics claiming one tag byte is a build error, not a review
/// finding.
#[must_use]
pub const fn tags_disjoint(tag_sets: &[&[u8]]) -> bool {
    let mut first = 0;
    while first < tag_sets.len() {
        let mut second = first + 1;
        while second < tag_sets.len() {
            let left = tag_sets[first];
            let right = tag_sets[second];
            let mut left_index = 0;
            while left_index < left.len() {
                let mut right_index = 0;
                while right_index < right.len() {
                    if left[left_index] == right[right_index] {
                        return false;
                    }
                    right_index += 1;
                }
                left_index += 1;
            }
            second += 1;
        }
        first += 1;
    }
    true
}

/// Declare the topic topology. Generates: a unit struct per topic
/// implementing [`WsTopic`], a `TopicId` enum (wire-name mapping,
/// `ALL`, `COUNT`, `bit()`), a `TopicSet` membership bitset, a static
/// vtable table with `vtable(id)` lookup, and a compile-time
/// assertion that no two topics claim the same binary tag.
///
/// ```ignore
/// define_ws_topics! {
///     registry Topics;
///     topic Events => "events" {
///         key: unkeyed, config: (), patch: NoPatch,
///         tags: [], control: false,
///     }
///     topic DisplayPreview => "display_preview" {
///         key: DeviceKey, config: DisplayPreviewConfig, patch: DisplayPreviewPatch,
///         tags: [0x07], control: false,
///     }
/// }
/// ```
#[macro_export]
macro_rules! define_ws_topics {
    (
        registry $registry:ident;
        $( topic $topic:ident => $name:literal {
            key: $key:tt, config: $config:ty, patch: $patch:ty,
            tags: [ $( $tag:literal ),* ], control: $control:literal $(,)?
        } )+
    ) => {
        $(
            #[doc = concat!("The `", $name, "` topic.")]
            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            pub struct $topic;

            impl $crate::ws::topic::WsTopic for $topic {
                const NAME: &'static str = $name;
                const OWNED_TAGS: &'static [u8] = &[ $( $tag ),* ];
                const REQUIRES_CONTROL: bool = $control;
                type Key = $crate::define_ws_topics!(@key_type $key);
                type Config = $config;
                type Patch = $patch;
            }
        )+

        /// Every topic in the registry, one variant per entry.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $registry {
            $( #[doc = concat!("`", $name, "`")] $topic, )+
        }

        impl $registry {
            /// Every topic, declaration order.
            pub const ALL: &'static [Self] = &[ $( Self::$topic, )+ ];

            /// Topic count.
            pub const COUNT: usize = Self::ALL.len();

            /// The wire name.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $( Self::$topic => $name, )+ }
            }

            /// Parse a wire name.
            #[must_use]
            pub fn parse(name: &str) -> Option<Self> {
                match name { $( $name => Some(Self::$topic), )+ _ => None }
            }

            /// The topic's membership bit (declaration-order discriminant).
            #[must_use]
            pub const fn bit(self) -> u32 {
                1 << (self as u32)
            }

            /// Whether subscribing requires the control tier.
            #[must_use]
            pub const fn requires_control(self) -> bool {
                match self {
                    $( Self::$topic => <$topic as $crate::ws::topic::WsTopic>::REQUIRES_CONTROL, )+
                }
            }

            /// The type-erased operations for this topic.
            #[must_use]
            pub fn vtable(self) -> &'static $crate::ws::topic::TopicVTable {
                match self {
                    $( Self::$topic => {
                        static VTABLE: std::sync::LazyLock<$crate::ws::topic::TopicVTable> =
                            std::sync::LazyLock::new(|| $crate::ws::topic::TopicVTable {
                                name: $name,
                                owned_tags: <$topic as $crate::ws::topic::WsTopic>::OWNED_TAGS,
                                requires_control: <$topic as $crate::ws::topic::WsTopic>::REQUIRES_CONTROL,
                                keyed: $crate::define_ws_topics!(@keyed $key),
                                // `()` serializes to null — configless by construction.
                                configurable: !serde_json::to_value(
                                    <<$topic as $crate::ws::topic::WsTopic>::Config as Default>::default()
                                ).unwrap_or(serde_json::Value::Null).is_null(),
                                default_config_json: || serde_json::to_value(
                                    <<$topic as $crate::ws::topic::WsTopic>::Config as Default>::default()
                                ).unwrap_or(serde_json::Value::Null),
                                validate_key: |key| {
                                    <<$topic as $crate::ws::topic::WsTopic>::Key as $crate::ws::topic::TopicKey>::from_wire(key).map(|_| ())
                                },
                                apply_patch_json: |config, patch| {
                                    let current: <$topic as $crate::ws::topic::WsTopic>::Config =
                                        serde_json::from_value(config.clone()).map_err(|error| {
                                            $crate::ws::topic::PatchError::new("config", error.to_string())
                                        })?;
                                    let patch: <$topic as $crate::ws::topic::WsTopic>::Patch =
                                        serde_json::from_value(patch.clone()).map_err(|error| {
                                            $crate::ws::topic::PatchError::new("patch", error.to_string())
                                        })?;
                                    let next = $crate::ws::topic::apply_patch_transactionally(&current, &patch)?;
                                    serde_json::to_value(next).map_err(|error| {
                                        $crate::ws::topic::PatchError::new("config", error.to_string())
                                    })
                                },
                            });
                        &VTABLE
                    } )+
                }
            }
        }

        /// Membership set over the registry's topics.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        pub struct TopicSet(u32);

        impl TopicSet {
            /// The empty set.
            pub const EMPTY: Self = Self(0);

            /// Insert a topic.
            pub fn insert(&mut self, topic: $registry) {
                self.0 |= topic.bit();
            }

            /// Remove a topic.
            pub fn remove(&mut self, topic: $registry) {
                self.0 &= !topic.bit();
            }

            /// Membership test.
            #[must_use]
            pub const fn contains(self, topic: $registry) -> bool {
                self.0 & topic.bit() != 0
            }

            /// Whether the set is empty.
            #[must_use]
            pub const fn is_empty(self) -> bool {
                self.0 == 0
            }

            /// Iterate members in declaration order.
            pub fn iter(self) -> impl Iterator<Item = $registry> {
                $registry::ALL.iter().copied().filter(move |topic| self.contains(*topic))
            }
        }

        // Two topics claiming one tag byte is a wire collision; make
        // it a build error rather than a review finding.
        const _: () = {
            assert!(
                $crate::ws::topic::tags_disjoint(&[
                    $( <$topic as $crate::ws::topic::WsTopic>::OWNED_TAGS, )+
                ]),
                "two topics claim the same binary tag"
            );
        };
    };

    (@key_type unkeyed) => { () };
    (@key_type $key:ty) => { $key };
    (@keyed unkeyed) => { false };
    (@keyed $key:ty) => { true };
}

/// A subscribe/unsubscribe selector on the wire: a topic name plus an
/// optional key. Keyed topics carry `key`; unkeyed topics omit it —
/// [`TopicKey::from_wire`] enforces both directions at the boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct TopicSelector {
    /// The topic's wire name.
    pub topic: String,
    /// The subscription key for keyed topics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Per-connection keyed subscription store: for each topic, the set of
/// live keys with their configs. Unkeyed topics use the empty-string
/// key slot internally; the wire never sees that detail.
#[derive(Debug, Clone, Default)]
pub struct SubscriptionTable {
    entries: BTreeMap<(u32, Option<String>), serde_json::Value>,
}

impl SubscriptionTable {
    /// Insert or replace a subscription's config.
    pub fn insert(&mut self, bit: u32, key: Option<String>, config: serde_json::Value) {
        self.entries.insert((bit, key), config);
    }

    /// Remove one subscription; returns whether it existed.
    pub fn remove(&mut self, bit: u32, key: Option<&str>) -> bool {
        self.entries
            .remove(&(bit, key.map(str::to_owned)))
            .is_some()
    }

    /// The config for one subscription.
    #[must_use]
    pub fn config(&self, bit: u32, key: Option<&str>) -> Option<&serde_json::Value> {
        self.entries.get(&(bit, key.map(str::to_owned)))
    }

    /// Every live key for a topic (empty when the topic has no
    /// subscription; one `None` entry when an unkeyed topic is live).
    pub fn keys_for(&self, bit: u32) -> impl Iterator<Item = Option<&str>> {
        self.entries
            .iter()
            .filter(move |((topic_bit, _), _)| *topic_bit == bit)
            .map(|((_, key), _)| key.as_deref())
    }

    /// Whether any subscription for the topic is live.
    #[must_use]
    pub fn any_for(&self, bit: u32) -> bool {
        self.keys_for(bit).next().is_some()
    }
}
