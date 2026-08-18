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
//! human descriptions, binary frame layouts, config bounds (which live
//! in the patch validators, not the config types), and the JSON message
//! name lists. Those are documentation and hand-written validation
//! ranges, not registry facts, so they have a home rather than being
//! transcribed into Rust.

use std::collections::BTreeMap;

use hypercolor_leptos_ext::ws::registry::TopicId;
use hypercolor_types::event::event_vocabulary;
use serde_json::{Map, Value, json};

use hypercolor_leptos_ext::ws::PREVIEW_MIN_MESSAGE_BYTES;

use super::protocol::{MAX_PREVIEW_PUBLICATION_BYTES, MAX_WS_MESSAGE_BYTES, ws_capabilities};

/// The authored half of the manifest, as committed.
pub const DESCRIPTIONS_JSON: &str =
    include_str!("../../../../../protocol/websocket-v1.descriptions.json");

/// Wire protocol version this manifest describes.
pub const PROTOCOL_VERSION: &str = "1.0";
/// Sec-WebSocket-Protocol value clients negotiate.
pub const SUBPROTOCOL: &str = "hypercolor-v1";
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
    manifest.insert("default_subscriptions".to_owned(), json!(["events"]));
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
        authored(&descriptions, "json_messages")?,
    );
    manifest.insert(
        "json_payloads".to_owned(),
        authored(&descriptions, "json_payloads")?,
    );
    manifest.insert(
        "binary_messages".to_owned(),
        binary_messages(&descriptions)?,
    );

    for block in FRAME_BLOCKS {
        manifest.insert((*block).to_owned(), authored(&descriptions, block)?);
    }
    manifest.insert("topic_config".to_owned(), topic_config(&descriptions)?);

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

/// The frame-layout blocks the descriptions file owns verbatim.
const FRAME_BLOCKS: &[&str] = &[
    "preview_frame",
    "zone_preview_frame",
    "interactive_preview_frame",
    "screen_zones_frame",
    "wide_preview_frame",
    "wide_zone_preview_frame",
    "wide_interactive_preview_frame",
    "wide_screen_zones_frame",
    "extended_screen_zones_frame",
    "preview_chunk_frame",
    "preview_cancel_frame",
    "display_preview_frame",
    "wide_display_preview_frame",
];

/// The no-replay contract, stated on the wire rather than only in the
/// agent notes (Spec 78 §7.1).
fn continuity() -> Value {
    json!({
        "events_replayed_across_reconnect": false,
        "contract":
            "The events channel carries live changes only. A client that \
             loses the socket misses every event during the gap, and the \
             daemon does not replay them on reconnect. Refetch the \
             resources you mirror whenever the socket opens — fold a \
             connection generation into your fetch epochs — and do the \
             same on resync_required, which the daemon sends when a \
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
        if let Some(fps) = default_config.get("fps").and_then(Value::as_u64) {
            entry.insert("default_fps".to_owned(), json!(fps));
        }
        if let Some(interval) = default_config.get("interval_ms").and_then(Value::as_u64) {
            entry.insert("default_interval_ms".to_owned(), json!(interval));
        }
        if let Some(description) = prose.get("description") {
            entry.insert("description".to_owned(), description.clone());
        }
        if vtable.requires_control {
            entry.insert("requires_control".to_owned(), json!(true));
        }
        if vtable.keyed {
            entry.insert("key".to_owned(), json!("required"));
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

fn binary_messages(descriptions: &Value) -> anyhow::Result<Value> {
    let authored = descriptions
        .get("binary_messages")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("descriptions file has no binary_messages object"))?;
    let owners = descriptions
        .get("binary_message_topics")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("descriptions file has no binary_message_topics object"))?;

    // Tags are the registry's, so a message can only claim a byte some
    // topic actually owns.
    let mut tag_owner: BTreeMap<u8, &'static str> = BTreeMap::new();
    for topic in TopicId::ALL {
        for tag in topic.vtable().owned_tags {
            tag_owner.insert(*tag, topic.vtable().name);
        }
    }

    let mut messages = Vec::with_capacity(authored.len());
    for (name, body) in authored {
        let mut entry = Map::new();
        entry.insert("name".to_owned(), json!(name));
        let tag = codec_tag(name)
            .ok_or_else(|| anyhow::anyhow!("binary message {name} has no declared tag"))?;
        entry.insert("tag".to_owned(), json!(tag));
        if let Some(body) = body.as_object() {
            for (key, value) in body {
                entry.insert(key.clone(), value.clone());
            }
        }
        if let Some(owner) = owners.get(name).and_then(Value::as_str) {
            entry.insert("topic".to_owned(), json!(owner));
        }
        messages.push((tag, Value::Object(entry)));
    }
    messages.sort_by_key(|(tag, _)| *tag);
    Ok(Value::Array(
        messages.into_iter().map(|(_, entry)| entry).collect(),
    ))
}

fn topic_config(descriptions: &Value) -> anyhow::Result<Value> {
    let authored = descriptions
        .get("topic_config")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("descriptions file has no topic_config object"))?;

    // Only configurable topics get a block, and the default in every
    // bound comes from the config type rather than the prose.
    let mut config = Map::new();
    for topic in TopicId::ALL {
        let vtable = topic.vtable();
        if !vtable.configurable {
            continue;
        }
        let name = vtable.name;
        let bounds = authored
            .get(name)
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow::anyhow!("configurable topic {name} has no authored bounds"))?;
        let defaults = (vtable.default_config_json)();

        let mut block = Map::new();
        for (field, bound) in bounds {
            let Some(bound) = bound.as_object() else {
                continue;
            };
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
    block.insert(
        "max_publication_decoded_bytes".to_owned(),
        json!(MAX_PREVIEW_PUBLICATION_BYTES),
    );
    block.insert("max_message_bytes".to_owned(), json!(MAX_WS_MESSAGE_BYTES));
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
