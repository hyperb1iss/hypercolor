//! Exercise view ownership against the real browser connection manager.
#![cfg(target_arch = "wasm32")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use hypercolor_ui::live_uptime::use_live_uptime;
use hypercolor_ui::ws::WsManager;
use hypercolor_ui::ws::telemetry::{TelemetryTopic, use_telemetry};
use hypercolor_ui::ws::transport::{
    WebSocketConnectRequest, WebSocketConnection, WebSocketEvent, WebSocketEventHandler,
    WebSocketMessage, WebSocketTransport, WebSocketTransportError, install_websocket_transport,
};
use leptos::prelude::*;
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(
    inline_js = "export function wait(ms) { return new Promise(resolve => setTimeout(resolve, ms)); }"
)]
extern "C" {
    fn wait(ms: u32) -> js_sys::Promise;
}

async fn settle() {
    JsFuture::from(wait(20)).await.expect("browser clock");
}

#[derive(Default)]
struct Probe {
    messages: RefCell<Vec<Value>>,
    events: RefCell<Option<WebSocketEventHandler>>,
    connections: Cell<usize>,
    fps_events: Cell<bool>,
}

impl Probe {
    fn deliver(&self, value: Value) {
        let events = self.events.borrow().clone().expect("socket is open");
        events(WebSocketEvent::Message(WebSocketMessage::Text(
            value.to_string(),
        )));
    }

    fn commands(&self, kind: &str, topic: &str) -> usize {
        self.messages
            .borrow()
            .iter()
            .filter(|message| {
                message["type"] == kind
                    && message["topics"]
                        .as_array()
                        .is_some_and(|topics| topics.iter().any(|entry| entry["topic"] == topic))
            })
            .count()
    }

    fn close(&self) {
        let events = self.events.borrow().clone().expect("socket is open");
        events(WebSocketEvent::Closed {
            code: 1006,
            reason: "test reconnect".into(),
        });
    }
}

struct Transport(Rc<Probe>);
struct Connection(Rc<Probe>);

impl WebSocketTransport for Transport {
    fn connect(
        &self,
        _: WebSocketConnectRequest,
        events: WebSocketEventHandler,
    ) -> Result<Rc<dyn WebSocketConnection>, WebSocketTransportError> {
        self.0.connections.set(self.0.connections.get() + 1);
        self.0.events.replace(Some(Rc::clone(&events)));
        events(WebSocketEvent::Opened);
        self.0.deliver(json!({
            "type": "hello",
            "capabilities": if self.0.fps_events.get() { vec!["fps_events"] } else { vec![] },
            "state": { "fps": { "target": 60 } }
        }));
        Ok(Rc::new(Connection(Rc::clone(&self.0))))
    }
}

impl WebSocketConnection for Connection {
    fn send(&self, message: WebSocketMessage) -> Result<(), WebSocketTransportError> {
        let WebSocketMessage::Text(text) = message else {
            panic!("expected JSON");
        };
        let message: Value = serde_json::from_str(&text).expect("JSON command");
        self.0.messages.borrow_mut().push(message.clone());
        if message["type"] == "subscribe" {
            let mut ack = json!({ "type": "subscribed", "topics": message["topics"] });
            if self.0.fps_events.get()
                && message["topics"]
                    .as_array()
                    .is_some_and(|topics| topics.iter().any(|topic| topic["topic"] == "events"))
            {
                ack["fps"] = json!({ "target": 45, "revision": 2 });
            }
            self.0.deliver(ack);
        }
        Ok(())
    }

    fn close(&self) -> Result<(), WebSocketTransportError> {
        Ok(())
    }
}

async fn reconnect(probe: &Probe) {
    let before = probe.connections.get();
    probe.close();
    for _ in 0..100 {
        settle().await;
        if probe.connections.get() > before {
            settle().await;
            return;
        }
    }
    panic!("the connection manager did not reconnect");
}

#[wasm_bindgen_test(async)]
async fn telemetry_follows_consumers_and_reconnects_without_clock_dependencies() {
    leptos::mount::mount_to_body(|| ());
    let probe = Rc::new(Probe::default());
    probe.fps_events.set(true);
    install_websocket_transport(Rc::new(Transport(Rc::clone(&probe)))).expect("install fixture");
    let root = Owner::new();
    let ws = root.with(WsManager::new);
    settle().await;
    assert_eq!(probe.commands("subscribe", "events"), 1);
    assert_eq!(probe.commands("subscribe", "metrics"), 0);
    assert_eq!(probe.commands("subscribe", "sensors"), 0);
    ws.set_preview_zone_resolved.set(true);
    ws.set_preview_cap.set(60);
    ws.set_preview_consumers.set(1);
    probe.deliver(json!({ "type": "event", "event": "fps_changed", "data": { "new_target": 60, "revision": 1 } }));
    settle().await;
    assert_eq!(
        ws.preview_target_fps.get_untracked(),
        45,
        "admission snapshot wins over queued older events"
    );

    let dashboard = root.with(|| {
        let owner = Owner::new();
        owner.with(|| {
            use_telemetry(TelemetryTopic::Metrics);
            use_telemetry(TelemetryTopic::Sensors);
        });
        owner
    });
    settle().await;
    assert_eq!(probe.commands("subscribe", "metrics"), 1);
    assert_eq!(probe.commands("subscribe", "sensors"), 1);
    assert!(
        probe
            .messages
            .borrow()
            .iter()
            .any(|message| message["topics"]
                == json!([
                    { "topic": "metrics", "config": { "fps": 2.0 } }
                ]))
    );
    reconnect(&probe).await;
    assert_eq!(probe.commands("subscribe", "metrics"), 2);
    assert_eq!(probe.commands("subscribe", "sensors"), 2);
    dashboard.cleanup();
    settle().await;
    assert_eq!(probe.commands("unsubscribe", "metrics"), 1);
    assert_eq!(probe.commands("unsubscribe", "sensors"), 1);

    ws.set_preview_zone_resolved.set(true);
    ws.set_preview_cap.set(60);
    ws.set_preview_consumers.set(1);
    probe.deliver(json!({ "type": "event", "event": "fps_changed", "data": { "new_target": 30, "measured": 1, "revision": 3 } }));
    settle().await;
    assert_eq!(
        ws.preview_target_fps.get_untracked(),
        30,
        "preview follows the target, not delivered FPS"
    );
    probe.deliver(json!({ "type": "backpressure", "topic": "canvas", "dropped_frames": 1, "recommendation": "reduce_fps", "suggested_fps": 15 }));
    settle().await;
    assert_eq!(ws.preview_target_fps.get_untracked(), 15);
    let clock_owner = root.with(Owner::new);
    let uptime = clock_owner.with(|| use_live_uptime(100));
    JsFuture::from(wait(2_100))
        .await
        .expect("recovery deadline");
    assert_eq!(
        ws.preview_target_fps.get_untracked(),
        30,
        "recovery does not need a metrics packet"
    );
    assert!(ws.backpressure_notice.get_untracked().is_none());
    assert!(
        uptime.get_untracked() >= 102,
        "uptime advances without network telemetry"
    );
    clock_owner.cleanup();

    probe.fps_events.set(false);
    reconnect(&probe).await;
    assert_eq!(
        probe.commands("subscribe", "metrics"),
        3,
        "older installations still provide FPS through metrics"
    );
    assert_eq!(
        probe.commands("subscribe", "sensors"),
        2,
        "unused sensors stay unsubscribed on older installations"
    );
    probe.deliver(json!({ "type": "metrics", "data": { "fps": { "target": 20 } } }));
    settle().await;
    assert_eq!(ws.preview_target_fps.get_untracked(), 20);
    probe.fps_events.set(true);
    reconnect(&probe).await;
    assert_eq!(
        probe.commands("subscribe", "metrics"),
        3,
        "new capabilities are read again after reconnect"
    );
    root.cleanup();
}
