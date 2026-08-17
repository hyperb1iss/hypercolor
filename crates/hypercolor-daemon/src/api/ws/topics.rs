//! Daemon-side runtime facts for each WS topic.
//!
//! The registry in `hypercolor-leptos-ext` owns what the wire agrees on:
//! names, keys, configs, patches, tags, gating. What a topic costs to
//! serve is local knowledge, and it lives here keyed by [`TopicId`]:
//! which relay task feeds it, and whether the daemon can afford the
//! surface a config asks for.
//!
//! Relays are registered per task, not per topic. Three topics share the
//! event relay because one bus subscription routes all three, and
//! `zone_preview` fans out to every live scene zone inside its own
//! relay. A table keyed by topic would have to invent a task per entry
//! and lose both.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::ws::Utf8Bytes;
use hypercolor_leptos_ext::ws::registry::{CanvasConfig, TopicId};
use serde::Deserialize;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::trace;

use super::protocol::{SubscriptionState, WsProtocolError, validate_passive_preview_shape};
use super::relays::{
    PreviewOutboundSender, relay_canvas, relay_device_metrics, relay_display_preview, relay_events,
    relay_frames, relay_metrics, relay_screen_canvas, relay_screen_zones, relay_sensors,
    relay_spectrum, relay_web_viewport_canvas, relay_zone_preview,
};
use crate::api::AppState;

/// Everything a relay task needs to attach itself to one connection.
pub(super) struct RelayContext {
    pub(super) state: Arc<AppState>,
    pub(super) json_tx: mpsc::Sender<Utf8Bytes>,
    pub(super) binary_tx: mpsc::Sender<Bytes>,
    pub(super) preview_tx: PreviewOutboundSender,
    pub(super) subscriptions: watch::Receiver<SubscriptionState>,
}

/// One relay task and the topics it serves.
struct RelayRegistration {
    /// Topics this task feeds. More than one means the task routes.
    topics: &'static [TopicId],
    spawn: fn(&RelayContext) -> JoinHandle<()>,
}

/// Every relay a connection spawns, once each.
static RELAYS: &[RelayRegistration] = &[
    RelayRegistration {
        // One bus subscription carries all three, and the relay routes by
        // event kind rather than opening three broadcast receivers.
        topics: &[TopicId::Events, TopicId::FrameEvents, TopicId::InputEvents],
        spawn: |context| {
            tokio::spawn(relay_events(
                context.state.event_bus.subscribe_all(),
                context.json_tx.clone(),
                context.subscriptions.clone(),
            ))
        },
    },
    RelayRegistration {
        topics: &[TopicId::Frames],
        spawn: |context| {
            tokio::spawn(relay_frames(
                Arc::clone(&context.state),
                context.json_tx.clone(),
                context.binary_tx.clone(),
                context.subscriptions.clone(),
            ))
        },
    },
    RelayRegistration {
        topics: &[TopicId::Spectrum],
        spawn: |context| {
            tokio::spawn(relay_spectrum(
                Arc::clone(&context.state),
                context.json_tx.clone(),
                context.binary_tx.clone(),
                context.subscriptions.clone(),
            ))
        },
    },
    RelayRegistration {
        topics: &[TopicId::Canvas],
        spawn: |context| {
            tokio::spawn(relay_canvas(
                Arc::clone(&context.state.preview_runtime),
                context.state.power_state.subscribe(),
                context.preview_tx.clone(),
                context.subscriptions.clone(),
            ))
        },
    },
    RelayRegistration {
        topics: &[TopicId::ScreenCanvas],
        spawn: |context| {
            tokio::spawn(relay_screen_canvas(
                Arc::clone(&context.state.preview_runtime),
                context.preview_tx.clone(),
                context.subscriptions.clone(),
            ))
        },
    },
    RelayRegistration {
        topics: &[TopicId::ScreenZones],
        spawn: |context| {
            tokio::spawn(relay_screen_zones(
                Arc::clone(&context.state.preview_runtime),
                context.subscriptions.clone(),
                context.preview_tx.clone(),
            ))
        },
    },
    RelayRegistration {
        topics: &[TopicId::WebViewportCanvas],
        spawn: |context| {
            tokio::spawn(relay_web_viewport_canvas(
                Arc::clone(&context.state.preview_runtime),
                context.preview_tx.clone(),
                context.subscriptions.clone(),
            ))
        },
    },
    RelayRegistration {
        // Fans out to one stream per live scene zone inside the relay.
        topics: &[TopicId::ZonePreview],
        spawn: |context| {
            tokio::spawn(relay_zone_preview(
                Arc::clone(&context.state.preview_runtime),
                context.preview_tx.clone(),
                context.subscriptions.clone(),
            ))
        },
    },
    RelayRegistration {
        topics: &[TopicId::DisplayPreview],
        spawn: |context| {
            tokio::spawn(relay_display_preview(
                Arc::clone(&context.state),
                Arc::clone(&context.state.display_frames),
                context.preview_tx.clone(),
                context.subscriptions.clone(),
            ))
        },
    },
    RelayRegistration {
        topics: &[TopicId::Metrics],
        spawn: |context| {
            tokio::spawn(relay_metrics(
                Arc::clone(&context.state),
                context.json_tx.clone(),
                context.subscriptions.clone(),
            ))
        },
    },
    RelayRegistration {
        topics: &[TopicId::DeviceMetrics],
        spawn: |context| {
            tokio::spawn(relay_device_metrics(
                Arc::clone(&context.state),
                context.json_tx.clone(),
                context.subscriptions.clone(),
            ))
        },
    },
    RelayRegistration {
        topics: &[TopicId::Sensors],
        spawn: |context| {
            tokio::spawn(relay_sensors(
                Arc::clone(&context.state),
                context.json_tx.clone(),
                context.subscriptions.clone(),
            ))
        },
    },
];

/// Spawn every relay this connection needs.
pub(super) fn spawn_relays(context: &RelayContext) -> Vec<JoinHandle<()>> {
    RELAYS
        .iter()
        .map(|registration| {
            trace!(topics = ?registration.topics, "Spawning WebSocket relay");
            (registration.spawn)(context)
        })
        .collect()
}

/// Admit a candidate config against the daemon's runtime budgets.
///
/// Runs immediately after the topic's patch applies, so a request with
/// two bad stanzas reports the earlier one, exactly as a single
/// hand-written validation pass did.
pub(super) fn admit_config(
    topic: TopicId,
    config: &serde_json::Value,
) -> Result<(), WsProtocolError> {
    match topic {
        TopicId::Canvas
        | TopicId::ScreenCanvas
        | TopicId::WebViewportCanvas
        | TopicId::ZonePreview => {
            let canvas = CanvasConfig::deserialize(config)
                .expect("a validated canvas config deserializes into its own type");
            validate_passive_preview_shape(&canvas, format!("config.{}", topic.as_str()))
        }
        TopicId::Frames
        | TopicId::Spectrum
        | TopicId::Events
        | TopicId::FrameEvents
        | TopicId::ScreenZones
        | TopicId::Metrics
        | TopicId::DeviceMetrics
        | TopicId::Sensors
        | TopicId::DisplayPreview
        | TopicId::InputEvents => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use hypercolor_leptos_ext::ws::registry::TopicId;

    use super::RELAYS;

    #[test]
    fn every_topic_is_served_by_exactly_one_relay() {
        let mut served = BTreeSet::new();
        for registration in RELAYS {
            for topic in registration.topics {
                assert!(
                    served.insert(*topic),
                    "{} is claimed by two relays",
                    topic.as_str()
                );
            }
        }

        let missing: Vec<&str> = TopicId::ALL
            .iter()
            .filter(|topic| !served.contains(topic))
            .map(|topic| topic.as_str())
            .collect();
        assert!(missing.is_empty(), "topics with no relay: {missing:?}");
    }

    #[test]
    fn relays_are_fewer_than_topics_because_the_event_relay_routes_three() {
        assert_eq!(RELAYS.len(), 12);
        assert_eq!(TopicId::COUNT, 14);
    }
}
