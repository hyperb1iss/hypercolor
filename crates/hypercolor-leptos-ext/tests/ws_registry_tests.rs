//! Contract coverage for the live WS topic topology (Spec 76 §5).
//!
//! Every assertion here pins something a client can observe: the wire
//! names and their order, the config JSON a fresh subscription echoes,
//! which topics take config at all, which need the control tier, and
//! which binary tags each topic owns. The tag-disjointness assertions
//! are compile-time, so this file merely COMPILING proves them.
#![cfg(feature = "ws-core")]

use std::collections::BTreeSet;

use hypercolor_leptos_ext::ws::registry::{
    CanvasConfig, CanvasConfigPatch, DisplayPreviewConfig, DisplayPreviewConfigPatch, FramesConfig,
    FramesConfigPatch, MetricsConfig, MetricsConfigPatch, SHARED_TRANSPORT_TAGS, SpectrumConfig,
    SpectrumConfigPatch, TopicId,
};
use hypercolor_leptos_ext::ws::topic::{TopicPatch, apply_patch_transactionally};
use serde_json::json;

/// Wire names in declaration order. The daemon's capability list and its
/// protocol manifest are both this sequence, so a reorder is a wire change.
const WIRE_NAMES: [&str; 14] = [
    "frames",
    "spectrum",
    "events",
    "frame_events",
    "canvas",
    "screen_canvas",
    "screen_zones",
    "web_viewport_canvas",
    "zone_preview",
    "metrics",
    "device_metrics",
    "sensors",
    "display_preview",
    "input_events",
];

#[test]
fn topic_wire_names_are_frozen_in_declaration_order() {
    let names: Vec<&str> = TopicId::ALL.iter().map(|topic| topic.as_str()).collect();
    assert_eq!(names, WIRE_NAMES);
    assert_eq!(TopicId::COUNT, WIRE_NAMES.len());
}

#[test]
fn every_wire_name_round_trips_through_parse() {
    for topic in TopicId::ALL.iter().copied() {
        assert_eq!(TopicId::parse(topic.as_str()), Some(topic));
    }
    assert_eq!(TopicId::parse("lasers"), None);
    assert_eq!(TopicId::parse("interactive_preview"), None);
}

#[test]
fn membership_bits_are_unique_across_the_registry() {
    let bits: BTreeSet<u32> = TopicId::ALL.iter().map(|topic| topic.bit()).collect();
    assert_eq!(bits.len(), TopicId::COUNT);
}

#[test]
fn control_tier_gates_screen_capture_and_host_input() {
    let gated: Vec<&str> = TopicId::ALL
        .iter()
        .filter(|topic| topic.requires_control())
        .map(|topic| topic.as_str())
        .collect();
    assert_eq!(gated, vec!["screen_canvas", "screen_zones", "input_events"]);
}

#[test]
fn configless_topics_carry_no_config_stanza() {
    let configless: Vec<&str> = TopicId::ALL
        .iter()
        .filter(|topic| !topic.vtable().configurable)
        .map(|topic| topic.as_str())
        .collect();
    assert_eq!(
        configless,
        vec![
            "events",
            "frame_events",
            "screen_zones",
            "sensors",
            "input_events"
        ]
    );
}

#[test]
fn default_configs_are_the_frozen_wire_defaults() {
    for (topic, expected) in [
        (
            TopicId::Frames,
            json!({"fps": 30, "format": "binary", "zones": ["all"]}),
        ),
        (TopicId::Spectrum, json!({"fps": 30, "bins": 64})),
        (
            TopicId::Canvas,
            json!({"fps": 15, "format": "rgb", "width": 0, "height": 0}),
        ),
        (
            TopicId::ScreenCanvas,
            json!({"fps": 15, "format": "rgb", "width": 0, "height": 0}),
        ),
        (
            TopicId::WebViewportCanvas,
            json!({"fps": 15, "format": "rgb", "width": 0, "height": 0}),
        ),
        (
            TopicId::ZonePreview,
            json!({"fps": 15, "format": "rgb", "width": 0, "height": 0}),
        ),
        (TopicId::Metrics, json!({"interval_ms": 1000})),
        (TopicId::DeviceMetrics, json!({"interval_ms": 1000})),
        (TopicId::DisplayPreview, json!({"fps": 15})),
    ] {
        assert_eq!(
            (topic.vtable().default_config_json)(),
            expected,
            "{} default config",
            topic.as_str()
        );
    }
}

#[test]
fn owned_and_shared_tags_cover_the_whole_binary_space() {
    let mut owned: Vec<u8> = TopicId::ALL
        .iter()
        .flat_map(|topic| topic.vtable().owned_tags.iter().copied())
        .collect();
    owned.sort_unstable();
    assert_eq!(
        owned,
        vec![
            0x01, 0x02, 0x03, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0c, 0x0e, 0x11
        ]
    );

    let mut all: Vec<u8> = owned;
    all.extend_from_slice(SHARED_TRANSPORT_TAGS);
    all.sort_unstable();
    assert_eq!(
        all,
        vec![
            0x01, 0x02, 0x03, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
            0x10, 0x11
        ],
        "0x04 stays unassigned; every other tag has exactly one home"
    );
}

#[test]
fn tag_owners_match_their_codecs() {
    assert_eq!(TopicId::Frames.vtable().owned_tags, [0x01]);
    assert_eq!(TopicId::Spectrum.vtable().owned_tags, [0x02]);
    assert_eq!(TopicId::Canvas.vtable().owned_tags, [0x03]);
    assert_eq!(TopicId::ScreenCanvas.vtable().owned_tags, [0x05]);
    assert_eq!(TopicId::WebViewportCanvas.vtable().owned_tags, [0x06]);
    assert_eq!(TopicId::DisplayPreview.vtable().owned_tags, [0x07]);
    assert_eq!(TopicId::ZonePreview.vtable().owned_tags, [0x08, 0x0c]);
    assert_eq!(TopicId::ScreenZones.vtable().owned_tags, [0x09, 0x0e, 0x11]);
    assert_eq!(TopicId::Metrics.vtable().owned_tags, [] as [u8; 0]);
}

#[test]
fn unkeyed_topics_reject_a_wire_key() {
    for topic in TopicId::ALL.iter().copied() {
        assert!(!topic.vtable().keyed, "{} is unkeyed", topic.as_str());
        assert_eq!((topic.vtable().validate_key)(None), Ok(None));
        assert!((topic.vtable().validate_key)(Some("anything")).is_err());
    }
}

#[test]
fn configless_topics_refuse_config_in_both_phases() {
    let vtable = TopicId::Sensors.vtable();

    // Non-null config fails while deserializing the patch.
    let deserialize_error = (vtable.apply_patch_json)(&json!(null), &json!({"fps": 10}))
        .expect_err("a configless topic takes no config object");
    assert_eq!(deserialize_error.field, "patch");

    // Explicit null deserializes, then fails on apply.
    let apply_error = (vtable.apply_patch_json)(&json!(null), &json!(null))
        .expect_err("a configless topic cannot apply a patch");
    assert_eq!(apply_error.field, "config");
    assert_eq!(apply_error.reason, "topic accepts no config");
}

#[test]
fn unknown_config_fields_are_rejected_rather_than_dropped() {
    let stale = json!({"fps": 30, "format": "binary", "zones": ["all"], "gone": true});
    let error = (TopicId::Frames.vtable().apply_patch_json)(&stale, &json!({"fps": 10}))
        .expect_err("a stale config field must fail loudly");
    assert_eq!(error.field, "config");
}

#[test]
fn unknown_patch_fields_are_rejected_rather_than_silently_ignored() {
    let error = (TopicId::Canvas.vtable().apply_patch_json)(
        &(TopicId::Canvas.vtable().default_config_json)(),
        &json!({"fpz": 30}),
    )
    .expect_err("a typo'd patch field must not succeed as a no-op");
    assert_eq!(error.field, "patch");
}

#[test]
fn frames_patch_validates_cadence_and_zone_selection() {
    let config = FramesConfig::default();

    for fps in [0_u32, 61] {
        let patch = FramesConfigPatch {
            fps: Some(fps),
            ..FramesConfigPatch::default()
        };
        let error = apply_patch_transactionally(&config, &patch).expect_err("fps out of range");
        assert_eq!(error.field, "fps");
        assert_eq!(error.reason, "expected 1..=60");
    }

    let empty = FramesConfigPatch {
        zones: Some(Vec::new()),
        ..FramesConfigPatch::default()
    };
    let error = apply_patch_transactionally(&config, &empty).expect_err("empty zones");
    assert_eq!(error.field, "zones");
    assert_eq!(error.reason, "must not be empty");

    let accepted = FramesConfigPatch {
        fps: Some(60),
        zones: Some(vec!["desk".to_owned()]),
        ..FramesConfigPatch::default()
    };
    let next = apply_patch_transactionally(&config, &accepted).expect("in-range patch applies");
    assert_eq!(next.fps, 60);
    assert_eq!(next.zones, vec!["desk".to_owned()]);
}

#[test]
fn spectrum_patch_admits_only_the_declared_bin_counts() {
    let config = SpectrumConfig::default();
    for bins in [8_u16, 16, 32, 64, 128] {
        let patch = SpectrumConfigPatch {
            bins: Some(bins),
            ..SpectrumConfigPatch::default()
        };
        assert_eq!(
            apply_patch_transactionally(&config, &patch)
                .expect("declared bin count")
                .bins,
            bins
        );
    }

    let patch = SpectrumConfigPatch {
        bins: Some(48),
        ..SpectrumConfigPatch::default()
    };
    let error = apply_patch_transactionally(&config, &patch).expect_err("undeclared bin count");
    assert_eq!(error.field, "bins");
    assert_eq!(error.reason, "expected one of [8, 16, 32, 64, 128]");
}

#[test]
fn metrics_patch_bounds_the_snapshot_period() {
    let config = MetricsConfig::default();
    for interval_ms in [99_u32, 10_001] {
        let patch = MetricsConfigPatch {
            interval_ms: Some(interval_ms),
        };
        let error = apply_patch_transactionally(&config, &patch).expect_err("interval bound");
        assert_eq!(error.field, "interval_ms");
        assert_eq!(error.reason, "expected 100..=10000");
    }
}

#[test]
fn canvas_patch_leaves_dimensions_unbounded_for_the_daemon_to_admit() {
    let config = CanvasConfig::default();
    let patch = CanvasConfigPatch {
        width: Some(u32::MAX),
        height: Some(0),
        ..CanvasConfigPatch::default()
    };
    let next = apply_patch_transactionally(&config, &patch).expect("wide shapes deserialize");
    assert_eq!((next.width, next.height), (u32::MAX, 0));
}

#[test]
fn a_failing_field_leaves_the_whole_config_untouched() {
    let config = CanvasConfig::default();
    let patch: CanvasConfigPatch =
        serde_json::from_value(json!({"width": 640, "fps": 240})).expect("patch parses");

    let error = apply_patch_transactionally(&config, &patch).expect_err("fps out of range");
    assert_eq!(error.field, "fps");
    assert_eq!(config, CanvasConfig::default());
}

#[test]
fn display_preview_target_is_a_tri_state() {
    let absent: DisplayPreviewConfigPatch =
        serde_json::from_value(json!({"fps": 10})).expect("fps-only patch parses");
    assert!(absent.device_id.is_none(), "missing key leaves the target");

    let cleared: DisplayPreviewConfigPatch =
        serde_json::from_value(json!({"device_id": null})).expect("null patch parses");
    assert_eq!(cleared.device_id, Some(None), "null clears the target");

    let set: DisplayPreviewConfigPatch =
        serde_json::from_value(json!({"device_id": "device-abc"})).expect("value patch parses");
    assert_eq!(set.device_id, Some(Some("device-abc".to_owned())));

    let mut config = DisplayPreviewConfig::default();
    set.apply(&mut config).expect("target applies");
    assert_eq!(config.device_id.as_deref(), Some("device-abc"));
    absent.apply(&mut config).expect("cadence applies");
    assert_eq!(config.device_id.as_deref(), Some("device-abc"));
    assert_eq!(config.fps, 10);
    cleared.apply(&mut config).expect("clear applies");
    assert!(config.device_id.is_none());
    assert_eq!(config.fps, 10);
}

#[test]
fn display_preview_target_must_name_a_real_device() {
    let patch: DisplayPreviewConfigPatch =
        serde_json::from_value(json!({"device_id": "   "})).expect("whitespace parses");
    let error = apply_patch_transactionally(&DisplayPreviewConfig::default(), &patch)
        .expect_err("whitespace is not a device");
    assert_eq!(error.field, "device_id");
    assert_eq!(error.reason, "must be non-empty when provided");

    let trimmed: DisplayPreviewConfigPatch =
        serde_json::from_value(json!({"device_id": " device-abc "})).expect("padded parses");
    let config = apply_patch_transactionally(&DisplayPreviewConfig::default(), &trimmed)
        .expect("padding is trimmed");
    assert_eq!(config.device_id.as_deref(), Some("device-abc"));
}

#[test]
fn display_preview_cadence_stops_at_thirty() {
    for fps in [0_u32, 31] {
        let patch = DisplayPreviewConfigPatch {
            fps: Some(fps),
            ..DisplayPreviewConfigPatch::default()
        };
        let error = apply_patch_transactionally(&DisplayPreviewConfig::default(), &patch)
            .expect_err("cadence bound");
        assert_eq!(error.field, "fps");
        assert_eq!(error.reason, "expected 1..=30");
    }
}

#[test]
fn a_detached_display_preview_omits_its_target_from_the_wire() {
    let config = DisplayPreviewConfig::default();
    assert_eq!(
        serde_json::to_value(&config).expect("config serializes"),
        json!({"fps": 15})
    );
}
