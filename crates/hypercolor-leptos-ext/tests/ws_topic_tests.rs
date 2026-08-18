//! End-to-end coverage of the topic registry machinery (Spec 76 §5)
//! over a proof topology that mirrors the real channel shapes: an
//! unkeyed configless topic, an unkeyed configured topic with a
//! tri-state patch, a control-tier topic, and a keyed topic. The
//! compile-time tag-disjointness assertion is exercised by this file
//! COMPILING — swap `FramesTopic`'s tag to `0x07` and the build fails.
#![cfg(feature = "ws-core")]
// The double-Option IS the tri-state wire pattern under test
// (missing = leave, null = clear, value = set).
#![allow(clippy::option_option)]

use hypercolor_leptos_ext::define_ws_topics;
use hypercolor_leptos_ext::ws::topic::{
    KeyError, NoPatch, PatchError, Subscription, SubscriptionTable, TopicKey, TopicPatch,
    apply_patch_transactionally,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceKey(String);

impl TopicKey for DeviceKey {
    fn to_wire(&self) -> Option<String> {
        Some(self.0.clone())
    }

    fn from_wire(key: Option<&str>) -> Result<Self, KeyError> {
        let key = key.ok_or(KeyError::MissingKey)?;
        let trimmed = key.trim();
        if trimmed.is_empty() || trimmed != key {
            return Err(KeyError::Invalid(
                "device key must be trimmed and non-empty".into(),
            ));
        }
        Ok(Self(key.to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FramesConfig {
    fps: u32,
    zones: Option<Vec<String>>,
}

impl Default for FramesConfig {
    fn default() -> Self {
        Self {
            fps: 30,
            zones: None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FramesPatch {
    fps: Option<u32>,
    // Tri-state: missing = leave, null = clear, list = set.
    #[serde(default, deserialize_with = "double_option")]
    zones: Option<Option<Vec<String>>>,
}

fn double_option<'de, D>(deserializer: D) -> Result<Option<Option<Vec<String>>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<Vec<String>>::deserialize(deserializer).map(Some)
}

impl TopicPatch<FramesConfig> for FramesPatch {
    fn apply(&self, config: &mut FramesConfig) -> Result<(), PatchError> {
        if let Some(fps) = self.fps {
            if !(1..=60).contains(&fps) {
                return Err(PatchError::new("fps", "must be 1..=60"));
            }
            config.fps = fps;
        }
        if let Some(zones) = &self.zones {
            config.zones.clone_from(zones);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewConfig {
    fps: u32,
}

impl Default for PreviewConfig {
    fn default() -> Self {
        Self { fps: 10 }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewPatch {
    fps: Option<u32>,
}

impl TopicPatch<PreviewConfig> for PreviewPatch {
    fn apply(&self, config: &mut PreviewConfig) -> Result<(), PatchError> {
        if let Some(fps) = self.fps {
            if !(1..=30).contains(&fps) {
                return Err(PatchError::new("fps", "must be 1..=30"));
            }
            config.fps = fps;
        }
        Ok(())
    }
}

define_ws_topics! {
    registry Topic;
    reserved [0x0F, 0x10];
    topic Events => "events" {
        key: unkeyed, config: (), patch: NoPatch,
        tags: [], control: false,
        backpressure: Lossless,
    }
    topic Frames => "frames" {
        key: unkeyed, config: FramesConfig, patch: FramesPatch,
        tags: [0x01], control: false,
        backpressure: DropWithNotice,
    }
    topic ScreenZones => "screen_zones" {
        key: unkeyed, config: (), patch: NoPatch,
        tags: [0x09, 0x0E, 0x11], control: true,
        backpressure: LatestWins,
    }
    topic DisplayPreview => "display_preview" {
        key: DeviceKey, config: PreviewConfig, patch: PreviewPatch,
        tags: [0x07, 0x0B], control: false,
        backpressure: LatestWins,
    }
}

#[test]
fn names_round_trip_and_order_is_declaration_order() {
    assert_eq!(Topic::COUNT, 4);
    for (index, topic) in Topic::ALL.iter().enumerate() {
        assert_eq!(Topic::parse(topic.as_str()), Some(*topic));
        assert_eq!(topic.bit(), 1 << index);
    }
    assert_eq!(Topic::parse("no_such_topic"), None);
    assert!(Topic::ScreenZones.requires_control());
    assert!(!Topic::Frames.requires_control());
}

#[test]
fn every_topic_declares_how_it_behaves_under_a_slow_reader() {
    use hypercolor_leptos_ext::ws::topic::BackpressureClass;

    // The class is mandatory by construction: the macro will not accept
    // an entry without one, so the manifest can always state it.
    assert_eq!(
        Topic::Events.vtable().backpressure,
        BackpressureClass::Lossless
    );
    assert_eq!(
        Topic::Frames.vtable().backpressure,
        BackpressureClass::DropWithNotice
    );
    assert_eq!(
        Topic::DisplayPreview.vtable().backpressure,
        BackpressureClass::LatestWins
    );
    assert_eq!(BackpressureClass::LatestWins.as_str(), "latest_wins");
}

#[test]
fn topic_set_membership_behaves() {
    let mut set = TopicSet::EMPTY;
    assert!(set.is_empty());
    set.insert(Topic::Frames);
    set.insert(Topic::DisplayPreview);
    assert!(set.contains(Topic::Frames));
    assert!(!set.contains(Topic::Events));
    let members: Vec<Topic> = set.iter().collect();
    assert_eq!(members, [Topic::Frames, Topic::DisplayPreview]);
    set.remove(Topic::Frames);
    assert!(!set.contains(Topic::Frames));
}

#[test]
fn vtable_reports_shape_facts() {
    let events = Topic::Events.vtable();
    assert!(!events.keyed);
    assert!(!events.configurable);
    assert!((events.default_config_json)().is_null());

    let frames = Topic::Frames.vtable();
    assert!(!frames.keyed);
    assert!(frames.configurable);
    assert_eq!((frames.default_config_json)()["fps"], 30);
    assert_eq!(frames.owned_tags, [0x01]);

    let display = Topic::DisplayPreview.vtable();
    assert!(display.keyed);
    assert_eq!(display.owned_tags, [0x07, 0x0B]);
}

#[test]
fn key_validation_rejects_both_mismatches_and_returns_canonical_form() {
    let display = Topic::DisplayPreview.vtable();
    assert_eq!(
        (display.validate_key)(Some("device-1")),
        Ok(Some("device-1".to_owned())),
        "callers store the canonical key, not the raw wire text"
    );
    assert_eq!((display.validate_key)(None), Err(KeyError::MissingKey));
    assert!(matches!(
        (display.validate_key)(Some("  padded ")),
        Err(KeyError::Invalid(_))
    ));

    let frames = Topic::Frames.vtable();
    assert_eq!((frames.validate_key)(None), Ok(None));
    assert_eq!(
        (frames.validate_key)(Some("unexpected")),
        Err(KeyError::UnexpectedKey)
    );
}

#[test]
fn patch_application_is_transactional() {
    let config = FramesConfig {
        fps: 24,
        zones: Some(vec!["left".into()]),
    };
    // A patch with one valid and one invalid field must change nothing.
    let bad = FramesPatch {
        fps: Some(0),
        zones: Some(None),
    };
    let error = apply_patch_transactionally(&config, &bad).expect_err("must fail");
    assert_eq!(error.field, "fps");
    assert_eq!(config.fps, 24, "original untouched");
    assert!(config.zones.is_some(), "original untouched");

    // The tri-state clear lands when the whole patch validates.
    let clear = FramesPatch {
        fps: Some(60),
        zones: Some(None),
    };
    let next = apply_patch_transactionally(&config, &clear).expect("applies");
    assert_eq!(next.fps, 60);
    assert!(next.zones.is_none());
}

#[test]
fn json_dispatch_round_trips_through_the_vtable() {
    let frames = Topic::Frames.vtable();
    let current = (frames.default_config_json)();
    let next = (frames.apply_patch_json)(&current, &serde_json::json!({"fps": 42}))
        .expect("valid patch applies");
    assert_eq!(next["fps"], 42);

    let error = (frames.apply_patch_json)(&current, &serde_json::json!({"fps": 900}))
        .expect_err("range violation");
    assert_eq!(error.field, "fps");

    // A typo'd field must be rejected, never silently dropped as an
    // empty patch (deny_unknown_fields is contractual on patch types).
    let typo = (frames.apply_patch_json)(&current, &serde_json::json!({"fpp": 42}))
        .expect_err("unknown fields are rejected, not dropped");
    assert_eq!(typo.field, "patch");

    // The tri-state clear survives JSON erasure: explicit null clears.
    let with_zones = (frames.apply_patch_json)(&current, &serde_json::json!({"zones": ["a"]}))
        .expect("set zones");
    assert_eq!(with_zones["zones"][0], "a");
    let cleared = (frames.apply_patch_json)(&with_zones, &serde_json::json!({"zones": null}))
        .expect("clear zones");
    assert!(cleared["zones"].is_null());

    // Configless topics reject any config payload.
    let events = Topic::Events.vtable();
    assert!(
        (events.apply_patch_json)(&serde_json::Value::Null, &serde_json::json!({"x": 1})).is_err()
    );
}

#[test]
#[allow(clippy::unit_arg)] // unit keys are exactly the contract under test
fn typed_subscriptions_make_invalid_states_unrepresentable() {
    let subscription = Subscription::<DisplayPreview> {
        key: DeviceKey::from_wire(Some("device-9")).expect("valid"),
        config: PreviewConfig { fps: 15 },
    };
    assert_eq!(subscription.key.to_wire().as_deref(), Some("device-9"));
    // An unkeyed topic's key is `()` — there is nothing to get wrong.
    let unkeyed = Subscription::<Frames> {
        key: (),
        config: FramesConfig::default(),
    };
    assert_eq!(unkeyed.key.to_wire(), None);
}

#[test]
fn subscription_table_holds_n_keys_per_topic() {
    let mut table = SubscriptionTable::default();
    let bit = Topic::DisplayPreview.bit();
    table.insert(bit, Some("dev-a".into()), serde_json::json!({"fps": 10}));
    table.insert(bit, Some("dev-b".into()), serde_json::json!({"fps": 20}));
    table.insert(Topic::Frames.bit(), None, serde_json::json!({"fps": 30}));

    let keys: Vec<_> = table.keys_for(bit).collect();
    assert_eq!(keys, [Some("dev-a"), Some("dev-b")]);
    assert!(table.any_for(bit));
    // The ack projection walks (key, config) pairs.
    let entries: Vec<_> = table.entries_for(bit).collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].0, Some("dev-b"));
    assert_eq!(entries[1].1["fps"], 20);
    assert_eq!(
        table.config(bit, Some("dev-b")).expect("present")["fps"],
        20
    );

    assert!(table.remove(bit, Some("dev-a")));
    assert!(!table.remove(bit, Some("dev-a")), "second remove is false");
    assert_eq!(table.keys_for(bit).count(), 1);
    // The other topic's unkeyed entry is untouched.
    assert!(table.any_for(Topic::Frames.bit()));
}
