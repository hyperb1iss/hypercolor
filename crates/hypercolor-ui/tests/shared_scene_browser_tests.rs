//! Exercise shared scene reads through the app's actual reactive consumers.
#![cfg(target_arch = "wasm32")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::task::{Poll, Waker};

use hypercolor_types::control::ControlValue;
use hypercolor_ui::api::client::{install_http_transport, install_verified_daemon_connection};
use hypercolor_ui::api::http_transport::{
    HttpRequest, HttpResponse, HttpTransport, HttpTransportFuture,
};
use hypercolor_ui::app::{EffectsContext, app_view};
use hypercolor_ui::extensions::UiExtensions;
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

const EFFECT: &str = "00000000-0000-4000-8000-000000000001";
const OTHER: &str = "00000000-0000-4000-8000-000000000002";
const ZONE: &str = "00000000-0000-4000-8000-000000000003";
const LAYER: &str = "00000000-0000-4000-8000-000000000004";

#[wasm_bindgen(
    inline_js = "export function wait() { return new Promise(resolve => setTimeout(resolve, 20)); }"
)]
extern "C" {
    fn wait() -> js_sys::Promise;
}
async fn settle() {
    JsFuture::from(wait()).await.expect("browser clock");
}

#[derive(Default)]
struct Gate {
    released: Cell<bool>,
    waker: RefCell<Option<Waker>>,
}
impl Gate {
    fn release(&self) {
        self.released.set(true);
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }
    async fn wait(&self) {
        futures_util::future::poll_fn(|cx| {
            if self.released.get() {
                Poll::Ready(())
            } else {
                self.waker.replace(Some(cx.waker().clone()));
                Poll::Pending
            }
        })
        .await;
    }
}

struct Fixture {
    scene: RefCell<Value>,
    name: RefCell<String>,
    requests: RefCell<Vec<String>>,
    gates: RefCell<Vec<(String, Rc<Gate>)>>,
    events: RefCell<Option<WebSocketEventHandler>>,
    connections: Cell<usize>,
    fail_apply: Cell<bool>,
    fail_scene: Cell<bool>,
}
impl Fixture {
    fn block(&self, path: &str) -> Rc<Gate> {
        let gate = Rc::new(Gate::default());
        self.gates.borrow_mut().push((path.into(), gate.clone()));
        gate
    }
    fn count(&self, path: &str) -> usize {
        self.requests
            .borrow()
            .iter()
            .filter(|p| p.as_str() == path)
            .count()
    }
    fn deliver(&self, value: Value) {
        let events = self.events.borrow().clone().expect("open socket");
        events(WebSocketEvent::Message(WebSocketMessage::Text(
            value.to_string(),
        )));
    }
    fn controls(&self, value: f64, revision: u64) {
        self.scene.replace(scene(EFFECT, value, revision));
        self.deliver(json!({"type":"event", "event":"zone_changed", "data": {
            "scene_id":"00000000-0000-4000-8000-000000000000", "zone_id":ZONE,
            "role":"primary", "kind":"controls_patched"
        }}));
    }
    fn registry(&self, name: &str) {
        self.name.replace(name.into());
        self.deliver(
            json!({"type":"event", "event":"effect_registry_updated", "data":{
                "added":0,"updated":1,"removed":0,"total":1
            }}),
        );
    }
    fn definition(&self, id: &str) -> Value {
        json!({"id":id,"name":*self.name.borrow(),"description":"","author":"test",
            "category":"ambient","source":"html","runnable":true,"tags":[],"version":"1",
            "audio_reactive":false,"controls":[{"id":"gain","name":*self.name.borrow(),
                "control_type":"slider","default_value":{"kind":"float","value":0.1}}]})
    }
}
impl HttpTransport for Fixture {
    fn send(&self, request: HttpRequest) -> HttpTransportFuture<'_> {
        let path = request.path;
        self.requests.borrow_mut().push(path.clone());
        let gate = {
            let mut gates = self.gates.borrow_mut();
            gates
                .iter()
                .position(|(p, _)| *p == path)
                .map(|i| gates.remove(i).1)
        };
        let (status, data) = if path == "/api/v1/scene" {
            if self.fail_scene.replace(false) {
                (500, json!(null))
            } else {
                (200, self.scene.borrow().clone())
            }
        } else if path == "/api/v1/effects" {
            (
                200,
                json!([self.definition(EFFECT), self.definition(OTHER)]),
            )
        } else if path.ends_with("/apply") {
            if self.fail_apply.get() {
                (500, json!(null))
            } else {
                (
                    200,
                    json!({"zone":self.scene.borrow()["zones"][0],"transition":{"type":"cut"},"output":{"applied":true}}),
                )
            }
        } else if let Some(id) = path.strip_prefix("/api/v1/effects/") {
            (200, self.definition(id))
        } else if path == "/api/v1/favorites" || path == "/api/v1/scenes" {
            (200, json!([]))
        } else {
            (404, json!(null))
        };
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.wait().await;
            }
            Ok(HttpResponse { status, headers:vec![], body:serde_json::to_vec(&json!({
                "data":data,"meta":{"api_version":"1.0","request_id":"test","timestamp":"1970-01-01T00:00:00Z"}
            })).unwrap() })
        })
    }
}
struct Transport(Rc<Fixture>);
struct Connection(Rc<Fixture>);
impl WebSocketTransport for Transport {
    fn connect(
        &self,
        _: WebSocketConnectRequest,
        events: WebSocketEventHandler,
    ) -> Result<Rc<dyn WebSocketConnection>, WebSocketTransportError> {
        self.0.connections.set(self.0.connections.get() + 1);
        self.0.events.replace(Some(events.clone()));
        events(WebSocketEvent::Opened);
        self.0.deliver(
            json!({"type":"hello","capabilities":["fps_events"],"state":{"fps":{"target":60}}}),
        );
        Ok(Rc::new(Connection(self.0.clone())))
    }
}
impl WebSocketConnection for Connection {
    fn send(&self, message: WebSocketMessage) -> Result<(), WebSocketTransportError> {
        if let WebSocketMessage::Text(text) = message {
            let message: Value = serde_json::from_str(&text).expect("JSON command");
            if message["type"] == "subscribe" {
                self.0
                    .deliver(json!({"type":"subscribed", "topics":message["topics"],
                    "fps":{"target":60,"revision":0}}));
            }
        }
        Ok(())
    }
    fn close(&self) -> Result<(), WebSocketTransportError> {
        Ok(())
    }
}
fn scene(effect: &str, value: f64, revision: u64) -> Value {
    json!({"id":"00000000-0000-4000-8000-000000000000","name":"Default","kind":"ephemeral",
    "is_default":true,"revision":revision,"zones":[{"id":ZONE,"name":"Main","role":"primary",
        "enabled":true,"brightness":1.0,"members":[],"layers":[{"id":LAYER,"source":{
            "type":"effect","effect_id":effect,"controls":{"gain":{"kind":"float","value":value}}
        }}]}]})
}
fn assert_value(ctx: EffectsContext, value: f64, revision: u64) {
    assert_eq!(
        ctx.active_control_values.get_untracked().get("gain"),
        Some(&ControlValue::Float(value))
    );
    assert_eq!(
        ctx.active_effect_target.get_untracked().unwrap().zone_id,
        ZONE
    );
    assert_eq!(
        ctx.zones.active_scene.get_untracked().unwrap().revision,
        revision
    );
}

#[wasm_bindgen_test(async)]
async fn shared_scene_drives_controls_and_rejects_stale_async_work() {
    leptos::mount::mount_to_body(|| ());
    let fixture = Rc::new(Fixture {
        scene: RefCell::new(scene(EFFECT, 0.1, 1)),
        name: RefCell::new("Original".into()),
        requests: RefCell::new(vec![]),
        gates: RefCell::new(vec![]),
        events: RefCell::new(None),
        connections: Cell::new(0),
        fail_apply: Cell::new(false),
        fail_scene: Cell::new(false),
    });
    install_verified_daemon_connection("http://127.0.0.1:9420", None);
    install_http_transport(fixture.clone()).unwrap();
    install_websocket_transport(Rc::new(Transport(fixture.clone()))).unwrap();
    let owner = Owner::new();
    let (_view, ctx) = owner.with(|| {
        let view = app_view(UiExtensions::default());
        (view, expect_context::<EffectsContext>())
    });
    let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    owner.with(|| {
        let observed = observed.clone();
        Effect::new(move |_| {
            observed
                .lock()
                .unwrap()
                .push(ctx.active_control_values.get().get("gain").cloned());
        });
    });
    settle().await;
    assert_value(ctx, 0.1, 1);
    fixture.requests.borrow_mut().clear();
    fixture.controls(0.2, 2);
    settle().await;
    assert_value(ctx, 0.2, 2);
    assert_eq!(
        fixture.count("/api/v1/scene"),
        1,
        "both consumers share one read"
    );
    assert_eq!(
        fixture.count(&format!("/api/v1/effects/{EFFECT}")),
        0,
        "control changes reuse definitions"
    );

    let old = fixture.block("/api/v1/scene");
    fixture.controls(0.3, 3);
    settle().await;
    fixture.controls(0.4, 4);
    settle().await;
    // LocalResource completes its current read before processing a queued
    // invalidation. The old response must never reach the primary controls.
    assert_value(ctx, 0.2, 2);
    old.release();
    settle().await;
    assert_value(ctx, 0.4, 4);
    assert!(
        !observed
            .lock()
            .unwrap()
            .contains(&Some(ControlValue::Float(0.3)))
    );

    let pending = fixture.block("/api/v1/scene");
    fixture.controls(0.5, 5);
    settle().await;
    fixture.registry("Edited during read");
    settle().await;
    pending.release();
    settle().await;
    assert_value(ctx, 0.5, 5);
    assert_eq!(
        ctx.active_controls.get_untracked()[0].name,
        "Edited during read"
    );

    let stale_definition = fixture.block(&format!("/api/v1/effects/{EFFECT}"));
    fixture.registry("Old definition response");
    settle().await;
    fixture.registry("New definition response");
    settle().await;
    stale_definition.release();
    settle().await;
    assert_eq!(
        ctx.active_controls.get_untracked()[0].name,
        "New definition response"
    );

    let old_scene = fixture.block("/api/v1/scene");
    fixture.controls(0.6, 6);
    settle().await;
    fixture.fail_apply.set(true);
    let apply = fixture.block(&format!("/api/v1/effects/{OTHER}/apply"));
    ctx.apply_effect_to(OTHER.into(), None);
    settle().await;
    old_scene.release();
    fixture.controls(0.7, 7);
    fixture.registry("Changed during failed apply");
    settle().await;
    assert_eq!(
        ctx.active_effect_id.get_untracked().as_deref(),
        Some(OTHER),
        "pending apply keeps optimism"
    );
    apply.release();
    settle().await;
    assert_value(ctx, 0.7, 7);
    assert_eq!(
        ctx.active_controls.get_untracked()[0].name,
        "Changed during failed apply"
    );

    let before_apply = fixture.block("/api/v1/scene");
    fixture.controls(0.8, 8);
    settle().await;
    fixture.fail_apply.set(false);
    fixture.scene.replace(scene(OTHER, 0.9, 9));
    ctx.apply_effect_to(OTHER.into(), None);
    settle().await;
    assert_eq!(ctx.active_effect_id.get_untracked().as_deref(), Some(OTHER));
    before_apply.release();
    settle().await;
    assert_value(ctx, 0.9, 9);
    assert_eq!(ctx.active_effect_id.get_untracked().as_deref(), Some(OTHER));

    fixture.scene.replace(scene(EFFECT, 1.0, 10));
    fixture.name.replace("After reconnect".into());
    let before = fixture.connections.get();
    let events = fixture.events.borrow().clone().unwrap();
    events(WebSocketEvent::Closed {
        code: 1006,
        reason: "test reconnect".into(),
    });
    for _ in 0..100 {
        settle().await;
        if fixture.connections.get() > before {
            break;
        }
    }
    assert!(fixture.connections.get() > before);
    settle().await;
    assert_value(ctx, 1.0, 10);
    assert_eq!(
        ctx.active_controls.get_untracked()[0].name,
        "After reconnect"
    );
    fixture.fail_scene.set(true);
    fixture.controls(1.1, 11);
    settle().await;
    assert_eq!(
        ctx.active_control_values.get_untracked()["gain"],
        ControlValue::Float(1.0)
    );
    fixture.controls(1.2, 12);
    settle().await;
    assert_value(ctx, 1.2, 12);

    let disposed = fixture.block(&format!("/api/v1/effects/{EFFECT}"));
    fixture.registry("Disposed response");
    settle().await;
    owner.cleanup();
    disposed.release();
    settle().await;
}
