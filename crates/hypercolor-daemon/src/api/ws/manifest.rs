//! The WebSocket protocol manifest, emitted from the topic registry
//! (Spec 78 §7.1).
//!
//! `protocol/websocket-v1.json` used to be hand-maintained input that
//! everything downstream trusted: the Python constant generator reads
//! it, the golden suite reads it, and the manifest-consistency test
//! compared it against the registry after the fact. Drift was possible
//! and only detectable.
//!
//! It is generated output now. Every fact about a topic — its name,
//! kind, cadence, control gate, key, backpressure class, and whether it
//! takes config — comes from `define_ws_topics!`, and the event
//! vocabulary comes from `HypercolorEvent`. Editing those in the JSON
//! accomplishes nothing: the next `just ws-manifest` overwrites them
//! and `just ws-manifest-check` fails in the meantime.
//!
//! What stays authored lives in `protocol/websocket-v1.descriptions.json`:
//! human topic and transport descriptions. Config bounds
//! come from the same compiled registry metadata as the validators, and
//! binary layouts come from the compiled codecs.

use std::collections::BTreeMap;

use hypercolor_leptos_ext::ws::registry::TopicId;
use hypercolor_types::event::event_vocabulary;
use serde_json::{Map, Value, json};

use hypercolor_leptos_ext::ws::{
    HYPERCOLOR_WS_PROTOCOL, HYPERCOLOR_WS_VERSION, PREVIEW_MIN_MESSAGE_BYTES,
    PreviewTransportCapability, codec_binary_messages, codec_frame_layouts,
};

use super::protocol::{
    SubscriptionState, client_message_vocabulary, json_payload_manifest, server_message_vocabulary,
    ws_capabilities,
};

/// The authored half of the manifest, as committed.
pub const DESCRIPTIONS_JSON: &str =
    include_str!("../../../../../protocol/websocket-v1.descriptions.json");

/// Wire protocol version this manifest describes.
pub const PROTOCOL_VERSION: &str = HYPERCOLOR_WS_VERSION;
/// Sec-WebSocket-Protocol value clients negotiate.
pub const SUBPROTOCOL: &str = HYPERCOLOR_WS_PROTOCOL;
/// Manifest schema version, bumped when the manifest's own shape moves.
pub const SCHEMA_VERSION: u64 = 2;

/// Build the manifest.
///
/// # Errors
///
/// Fails when the authored descriptions file is malformed or is missing
/// an entry for a topic or binary message the registry declares — a
/// missing description is a gap in the committed documentation, not a
/// reason to emit a manifest with holes in it.
pub fn build() -> anyhow::Result<Value> {
    let descriptions: Value = serde_json::from_str(DESCRIPTIONS_JSON)?;
    let mut manifest = Map::new();

    manifest.insert("schema_version".to_owned(), json!(SCHEMA_VERSION));
    manifest.insert("protocol".to_owned(), json!("hypercolor.websocket"));
    manifest.insert("version".to_owned(), json!(PROTOCOL_VERSION));
    manifest.insert("subprotocol".to_owned(), json!(SUBPROTOCOL));
    let default_subscriptions = SubscriptionState::default()
        .live_subscriptions()
        .map(|subscription| subscription.topic.as_str())
        .collect::<Vec<_>>();
    manifest.insert(
        "default_subscriptions".to_owned(),
        json!(default_subscriptions),
    );
    manifest.insert("topics".to_owned(), topics(&descriptions)?);
    manifest.insert("events".to_owned(), json!(event_vocabulary()));
    manifest.insert("continuity".to_owned(), continuity());
    manifest.insert("capabilities".to_owned(), json!(ws_capabilities()));
    manifest.insert(
        "preview_transport".to_owned(),
        preview_transport(&descriptions)?,
    );
    manifest.insert(
        "json_messages".to_owned(),
        json!({
            "client": client_message_vocabulary(),
            "server": server_message_vocabulary(),
        }),
    );
    manifest.insert("json_payloads".to_owned(), json_payload_manifest());
    manifest.insert("binary_messages".to_owned(), binary_messages()?);

    for (name, block) in codec_frame_layouts() {
        manifest.insert(name, block);
    }
    manifest.insert("topic_config".to_owned(), topic_config()?);

    Ok(Value::Object(manifest))
}

/// Serialize the manifest exactly as it is committed.
///
/// # Errors
///
/// As [`build`], plus a serialization failure.
pub fn build_json() -> anyhow::Result<String> {
    Ok(format!("{}\n", serde_json::to_string_pretty(&build()?)?))
}

/// The no-replay contract, stated on the wire rather than only in the
/// agent notes (Spec 78 §7.1).
fn continuity() -> Value {
    json!({
        "events_replayed_across_reconnect": false,
        "contract":
            "The events channel carries live changes only. A client that \
             loses the socket misses every event during the gap, and the \
             daemon does not replay them on reconnect. Refetch the \
             resources you mirror after the first subscribed acknowledgment \
             on every connection, after the event relay is live. Fold that \
             subscription generation into your fetch epochs, and do the same \
             on resync_required, which the daemon sends when a \
             subscriber falls far enough behind that events were dropped \
             on a socket that is still open.",
        "resync_signal": "resync_required",
    })
}

fn topics(descriptions: &Value) -> anyhow::Result<Value> {
    let authored = descriptions
        .get("topics")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("descriptions file has no topics object"))?;

    let mut topics = Vec::with_capacity(TopicId::ALL.len());
    for topic in TopicId::ALL {
        let vtable = topic.vtable();
        let name = vtable.name;
        let prose = authored
            .get(name)
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow::anyhow!("topic {name} has no authored description"))?;

        let mut entry = Map::new();
        entry.insert("name".to_owned(), json!(name));
        entry.insert(
            "kind".to_owned(),
            json!(if vtable.owned_tags.is_empty() {
                "json"
            } else {
                "binary"
            }),
        );
        let default_config = (vtable.default_config_json)();
        if let Some(fps) = default_config.get("fps") {
            entry.insert("default_fps".to_owned(), fps.clone());
        }
        if let Some(description) = prose.get("description") {
            entry.insert("description".to_owned(), description.clone());
        }
        if vtable.requires_control {
            entry.insert("requires_control".to_owned(), json!(true));
        }
        if let Some(key_name) = vtable.key_name {
            entry.insert("key".to_owned(), json!(key_name));
        }
        entry.insert(
            "backpressure".to_owned(),
            json!(vtable.backpressure.as_str()),
        );
        if let Some(schema) = prose.get("payload_schema") {
            entry.insert("payload_schema".to_owned(), schema.clone());
        }
        topics.push(Value::Object(entry));
    }
    Ok(Value::Array(topics))
}

fn binary_messages() -> anyhow::Result<Value> {
    let mut codecs = codec_binary_messages();
    codecs.insert(
        "led_frame".to_owned(),
        super::cache::led_frame_codec_manifest(),
    );

    let mut tag_owner: BTreeMap<u8, &'static str> = BTreeMap::new();
    for topic in TopicId::ALL {
        for tag in topic.vtable().owned_tags {
            tag_owner.insert(*tag, topic.vtable().name);
        }
    }

    let mut messages = Vec::with_capacity(codecs.len());
    for (name, body) in codecs {
        let mut entry = Map::new();
        let tag = codec_tag(&name)
            .ok_or_else(|| anyhow::anyhow!("binary message {name} has no declared tag"))?;
        if let Some(body) = body.as_object() {
            for (key, value) in body {
                if !matches!(key.as_str(), "name" | "tag" | "topic") {
                    entry.insert(key.clone(), value.clone());
                }
            }
        } else {
            entry.insert("layout".to_owned(), body);
        }
        let owner = tag_owner.get(&tag).copied().or_else(|| {
            TopicId::RESERVED_TAGS
                .contains(&tag)
                .then_some("preview_transport")
        });
        let owner = owner
            .ok_or_else(|| anyhow::anyhow!("binary message {name} has unowned tag {tag:#04x}"))?;
        entry.insert("name".to_owned(), json!(&name));
        entry.insert("tag".to_owned(), json!(tag));
        entry.insert("topic".to_owned(), json!(owner));
        messages.push((tag, Value::Object(entry)));
    }
    messages.sort_by_key(|(tag, _)| *tag);
    Ok(Value::Array(
        messages.into_iter().map(|(_, entry)| entry).collect(),
    ))
}

fn topic_config() -> anyhow::Result<Value> {
    let mut config = Map::new();
    for topic in TopicId::ALL {
        let vtable = topic.vtable();
        if !vtable.configurable {
            continue;
        }
        let name = vtable.name;
        let bounds_value = (vtable.config_schema_json)();
        let bounds = bounds_value
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("configurable topic {name} has no compiled bounds"))?;
        let defaults = (vtable.default_config_json)();
        let default_fields = defaults
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("configurable topic {name} has no config object"))?;
        let missing = default_fields
            .keys()
            .filter(|field| !bounds.contains_key(*field))
            .map(String::as_str)
            .collect::<Vec<_>>();
        let extra = bounds
            .keys()
            .filter(|field| !default_fields.contains_key(*field))
            .map(String::as_str)
            .collect::<Vec<_>>();
        if !missing.is_empty() || !extra.is_empty() {
            anyhow::bail!(
                "configurable topic {name} bounds mismatch: missing {missing:?}, extra {extra:?}"
            );
        }

        let mut block = Map::new();
        for (field, bound) in bounds {
            let bound = bound.as_object().ok_or_else(|| {
                anyhow::anyhow!("configurable topic {name} field {field} bounds must be an object")
            })?;
            let mut merged = bound.clone();
            if let Some(default) = defaults.get(field) {
                merged.insert("default".to_owned(), default.clone());
            }
            block.insert(field.clone(), Value::Object(merged));
        }
        config.insert(name.to_owned(), Value::Object(block));
    }
    Ok(Value::Object(config))
}

fn preview_transport(descriptions: &Value) -> anyhow::Result<Value> {
    let mut block = authored(descriptions, "preview_transport")?
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("preview_transport must be an object"))?;
    let defaults = PreviewTransportCapability::default();
    block.insert(
        "max_publication_decoded_bytes".to_owned(),
        json!(defaults.max_decoded_publication_bytes),
    );
    block.insert(
        "max_publication_encoded_bytes".to_owned(),
        json!(defaults.max_encoded_publication_bytes),
    );
    block.insert(
        "max_connection_bytes".to_owned(),
        json!(defaults.max_connection_bytes),
    );
    block.insert("partial_idle_ms".to_owned(), json!(defaults.max_idle_ms));
    block.insert(
        "max_message_bytes".to_owned(),
        json!(defaults.max_message_bytes),
    );
    block.insert(
        "max_reassembly_state_bytes".to_owned(),
        json!(defaults.max_reassembly_state_bytes),
    );
    block.insert(
        "max_tombstone_bytes".to_owned(),
        json!(defaults.max_tombstone_bytes),
    );
    block.insert(
        "max_sender_state_bytes".to_owned(),
        json!(defaults.max_sender_state_bytes),
    );
    block.insert(
        "max_cursor_state_bytes".to_owned(),
        json!(defaults.max_cursor_state_bytes),
    );
    block.insert(
        "min_message_bytes".to_owned(),
        json!(PREVIEW_MIN_MESSAGE_BYTES),
    );
    block.insert("jpeg_max_axis".to_owned(), json!(u16::MAX));
    Ok(Value::Object(block))
}

/// The wire tag each binary message carries.
///
/// Every value is a symbol rather than a literal: the preview family's
/// tags are the codec constants `hypercolor-leptos-ext` declares, and
/// the four topic-owned tags come from the registry entry that owns
/// them. A tag cannot drift between the encoder and the manifest
/// because there is only one of it.
fn codec_tag(name: &str) -> Option<u8> {
    use hypercolor_leptos_ext::ws::{
        DISPLAY_PREVIEW_FRAME_TAG, EXTENDED_SCREEN_ZONES_FRAME_TAG, INTERACTIVE_PREVIEW_FRAME_TAG,
        PREVIEW_CANCEL_FRAME_TAG, PREVIEW_CHUNK_FRAME_TAG, SCREEN_ZONES_FRAME_TAG,
        SPECTRUM_FRAME_TAG, WIDE_DISPLAY_PREVIEW_FRAME_TAG, WIDE_INTERACTIVE_PREVIEW_FRAME_TAG,
        WIDE_PREVIEW_FRAME_TAG, WIDE_SCREEN_ZONES_FRAME_TAG, WIDE_ZONE_PREVIEW_FRAME_TAG,
        ZONE_PREVIEW_FRAME_TAG,
    };

    let owned = |topic: TopicId| topic.vtable().owned_tags.first().copied();
    match name {
        "led_frame" => owned(TopicId::Frames),
        "spectrum" => Some(SPECTRUM_FRAME_TAG),
        "canvas" => owned(TopicId::Canvas),
        "screen_canvas" => owned(TopicId::ScreenCanvas),
        "web_viewport_canvas" => owned(TopicId::WebViewportCanvas),
        "screen_zones" => Some(SCREEN_ZONES_FRAME_TAG),
        "zone_preview" => Some(ZONE_PREVIEW_FRAME_TAG),
        "display_preview" => Some(DISPLAY_PREVIEW_FRAME_TAG),
        "interactive_preview" => Some(INTERACTIVE_PREVIEW_FRAME_TAG),
        "wide_preview" => Some(WIDE_PREVIEW_FRAME_TAG),
        "wide_zone_preview" => Some(WIDE_ZONE_PREVIEW_FRAME_TAG),
        "wide_interactive_preview" => Some(WIDE_INTERACTIVE_PREVIEW_FRAME_TAG),
        "wide_screen_zones" => Some(WIDE_SCREEN_ZONES_FRAME_TAG),
        "wide_display_preview" => Some(WIDE_DISPLAY_PREVIEW_FRAME_TAG),
        "extended_screen_zones" => Some(EXTENDED_SCREEN_ZONES_FRAME_TAG),
        "preview_chunk" => Some(PREVIEW_CHUNK_FRAME_TAG),
        "preview_cancel" => Some(PREVIEW_CANCEL_FRAME_TAG),
        _ => None,
    }
}

fn authored(descriptions: &Value, key: &str) -> anyhow::Result<Value> {
    descriptions
        .get(key)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("descriptions file has no {key} entry"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyed_topic_metadata_names_the_wire_identity() {
        let manifest = build().expect("manifest should build");
        let topics = manifest["topics"].as_array().expect("topics array");
        let key = |name: &str| {
            topics
                .iter()
                .find(|topic| topic["name"] == name)
                .and_then(|topic| topic["key"].as_str())
        };

        assert_eq!(key("display_preview"), Some("device_id"));
        assert_eq!(key("interactive_preview"), Some("preview_id"));
        assert_eq!(key("events"), None);
    }

    #[test]
    fn binary_message_owners_come_from_the_registry() {
        let messages = binary_messages().expect("binary messages should build");
        let messages = messages
            .as_array()
            .expect("binary messages should be an array");
        let owner = |name: &str| {
            messages
                .iter()
                .find(|message| message["name"] == name)
                .and_then(|message| message["topic"].as_str())
        };

        assert_eq!(owner("canvas"), Some("canvas"));
        assert_eq!(owner("preview_chunk"), Some("preview_transport"));
        let canvas = messages
            .iter()
            .find(|message| message["name"] == "canvas")
            .expect("canvas message should exist");
        assert_eq!(canvas["tag"], TopicId::Canvas.vtable().owned_tags[0]);

        let expected = [
            ("led_frame", 0x01, "frames"),
            ("spectrum", 0x02, "spectrum"),
            ("canvas", 0x03, "canvas"),
            ("screen_canvas", 0x05, "screen_canvas"),
            ("web_viewport_canvas", 0x06, "web_viewport_canvas"),
            ("display_preview", 0x07, "display_preview"),
            ("zone_preview", 0x08, "zone_preview"),
            ("screen_zones", 0x09, "screen_zones"),
            ("interactive_preview", 0x0a, "interactive_preview"),
            ("wide_preview", 0x0b, "preview_transport"),
            ("wide_zone_preview", 0x0c, "zone_preview"),
            ("wide_interactive_preview", 0x0d, "interactive_preview"),
            ("wide_screen_zones", 0x0e, "screen_zones"),
            ("preview_chunk", 0x0f, "preview_transport"),
            ("preview_cancel", 0x10, "preview_transport"),
            ("extended_screen_zones", 0x11, "screen_zones"),
            ("wide_display_preview", 0x12, "display_preview"),
        ];
        let actual = messages
            .iter()
            .map(|message| {
                (
                    message["name"].as_str().expect("codec name"),
                    message["tag"].as_u64().expect("codec tag") as u8,
                    message["topic"].as_str().expect("codec owner"),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);

        let registry_owned = TopicId::ALL
            .iter()
            .flat_map(|topic| {
                topic
                    .vtable()
                    .owned_tags
                    .iter()
                    .map(|tag| (*tag, topic.as_str()))
            })
            .collect::<BTreeMap<_, _>>();
        let manifested_owned = actual
            .iter()
            .filter(|(_, _, owner)| *owner != "preview_transport")
            .map(|(_, tag, owner)| (*tag, *owner))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(manifested_owned, registry_owned);
    }

    #[test]
    fn compiled_topic_config_bounds_match_runtime_validation() {
        let manifest = topic_config().expect("compiled topic config should build");

        for topic in TopicId::ALL {
            let vtable = topic.vtable();
            if !vtable.configurable {
                continue;
            }
            let defaults = (vtable.default_config_json)();
            let default_fields = defaults
                .as_object()
                .expect("configured topic default should be an object");
            let fields = manifest[vtable.name]
                .as_object()
                .expect("configured topic should have a manifest block");
            assert_eq!(
                fields.keys().collect::<Vec<_>>(),
                default_fields.keys().collect::<Vec<_>>(),
                "{} schema fields must exactly match its config type",
                vtable.name
            );

            for (field, bounds) in fields {
                let bounds = bounds
                    .as_object()
                    .expect("compiled field bounds should be an object");
                let apply = |value: Value| {
                    let mut patch = Map::new();
                    patch.insert(field.clone(), value);
                    (vtable.apply_patch_json)(&defaults, &Value::Object(patch))
                };

                if let Some(min) = bounds.get("min").and_then(Value::as_u64) {
                    assert!(apply(json!(min)).is_ok(), "{}.{} min", vtable.name, field);
                    if min > 0 {
                        assert!(
                            apply(json!(min - 1)).is_err(),
                            "{}.{} below min",
                            vtable.name,
                            field
                        );
                    }
                }
                if let Some(max) = bounds.get("max").and_then(Value::as_u64) {
                    assert!(apply(json!(max)).is_ok(), "{}.{} max", vtable.name, field);
                    assert!(
                        apply(json!(max + 1)).is_err(),
                        "{}.{} above max",
                        vtable.name,
                        field
                    );
                }
                if let Some(values) = bounds.get("values").and_then(Value::as_array) {
                    for value in values {
                        assert!(
                            apply(value.clone()).is_ok(),
                            "{}.{} advertised value {value}",
                            vtable.name,
                            field
                        );
                    }
                    let invalid = if values.iter().all(Value::is_number) {
                        json!(values.iter().filter_map(Value::as_u64).max().unwrap_or(0) + 1)
                    } else {
                        json!("__not_allowed__")
                    };
                    assert!(
                        apply(invalid).is_err(),
                        "{}.{} must reject an unadvertised value",
                        vtable.name,
                        field
                    );
                }
                if let Some(min_items) = bounds.get("min_items").and_then(Value::as_u64) {
                    assert!(min_items > 0, "{}.{} min_items", vtable.name, field);
                    assert!(
                        apply(Value::Array(Vec::new())).is_err(),
                        "{}.{} below min_items",
                        vtable.name,
                        field
                    );
                    assert!(
                        apply(defaults[field].clone()).is_ok(),
                        "{}.{} default satisfies min_items",
                        vtable.name,
                        field
                    );
                }
            }
        }
    }
}
