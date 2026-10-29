use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use hypercolor_types::control::ControlValue;
use hypercolor_ui::api::client::{install_http_transport, install_verified_daemon_connection};
use hypercolor_ui::api::http_transport::{
    HttpRequest, HttpResponse, HttpTransport, HttpTransportFuture,
};
use hypercolor_ui::api::{PrimaryEffectView, fetch_primary_effect_view_with_metadata};
use hypercolor_ui::app::effect_refresh::PrimaryEffectRefresh;
use serde_json::{Value, json};

const EFFECT: &str = "00000000-0000-4000-8000-000000000001";
const OTHER: &str = "00000000-0000-4000-8000-000000000002";
const ZONE: &str = "00000000-0000-4000-8000-000000000003";
const LAYER: &str = "00000000-0000-4000-8000-000000000004";
const PRESET: &str = "00000000-0000-4000-8000-000000000005";

struct Fixture {
    scene: RefCell<Value>,
    definition_name: RefCell<String>,
    requests: RefCell<Vec<String>>,
}

impl HttpTransport for Fixture {
    fn send(&self, request: HttpRequest) -> HttpTransportFuture<'_> {
        self.requests.borrow_mut().push(request.path.clone());
        let data = if request.path == "/api/v1/scene" {
            self.scene.borrow().clone()
        } else {
            let id = request
                .path
                .strip_prefix("/api/v1/effects/")
                .expect("effect detail path");
            json!({
                "id": id, "name": *self.definition_name.borrow(), "description": "",
                "author": "test", "category": "ambient", "source": "html",
                "runnable": true, "tags": [], "version": "1", "audio_reactive": false,
                "controls": [{
                    "id": "gain", "name": *self.definition_name.borrow(),
                    "control_type": "slider", "default_value": {"kind": "float", "value": 0.1}
                }]
            })
        };
        Box::pin(async move {
            Ok(HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: serde_json::to_vec(&json!({
                    "data": data,
                    "meta": {"api_version": "1.0", "request_id": "test", "timestamp": "1970-01-01T00:00:00Z"}
                })).expect("valid response"),
            })
        })
    }
}

fn scene(effect: &str, layer: &str, value: f64, preset: Option<&str>, revision: u64) -> Value {
    json!({
        "id": "00000000-0000-4000-8000-000000000000", "name": "Default",
        "kind": "ephemeral", "is_default": true, "revision": revision,
        "zones": [{
            "id": ZONE, "name": "Main", "role": "primary", "enabled": true,
            "brightness": 1.0, "members": [], "layers": [{
                "id": layer, "source": {
                    "type": "effect", "effect_id": effect,
                    "controls": {"gain": {"kind": "float", "value": value}},
                    "preset_id": preset
                }
            }]
        }]
    })
}

fn ready<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("fixture unexpectedly yielded"),
    }
}

fn reconcile(state: &mut PrimaryEffectRefresh, full: bool) -> Option<PrimaryEffectView> {
    let (generation, metadata) = state.begin(full);
    let view = ready(fetch_primary_effect_view_with_metadata(metadata)).expect("snapshot fetched");
    assert!(state.finish(generation, view.as_ref()));
    view
}

#[test]
fn control_reconciliation_fetches_live_values_and_reuses_only_matching_metadata() {
    let fixture = Rc::new(Fixture {
        scene: RefCell::new(scene(EFFECT, LAYER, 0.1, None, 1)),
        definition_name: RefCell::new("Original definition".to_owned()),
        requests: RefCell::new(Vec::new()),
    });
    install_verified_daemon_connection("http://127.0.0.1:9420", None);
    install_http_transport(fixture.clone()).expect("fixture installed");
    let mut state = PrimaryEffectRefresh::default();
    let first = reconcile(&mut state, true).expect("initial effect");
    assert_eq!(fixture.requests.borrow().len(), 2);

    // Another client changes values, preset and layer identity, keeping the effect.
    *fixture.scene.borrow_mut() = scene(EFFECT, OTHER, 0.8, Some(PRESET), 2);
    let changed = reconcile(&mut state, false).expect("changed effect");
    assert_eq!(
        fixture.requests.borrow().len(),
        3,
        "only the live scene is fetched"
    );
    assert_eq!(changed.control_values["gain"], ControlValue::Float(0.8));
    assert_eq!(changed.active_preset_id.as_deref(), Some(PRESET));
    assert_eq!(changed.layer_id, OTHER);
    assert_eq!(changed.zone_id, ZONE);
    assert_eq!(changed.scene_revision, 2);
    assert_eq!(changed.controls, first.controls);

    // Even a control-only hint must fetch definitions if the target effect changed.
    *fixture.scene.borrow_mut() = scene(OTHER, LAYER, 0.4, None, 3);
    *fixture.definition_name.borrow_mut() = "Other effect".to_owned();
    assert_eq!(
        reconcile(&mut state, false).expect("other effect").name,
        "Other effect"
    );
    assert_eq!(fixture.requests.borrow().len(), 5);

    // Structural refreshes and reconnects invalidate even the same effect's schema.
    *fixture.definition_name.borrow_mut() = "Edited definition".to_owned();
    let edited = reconcile(&mut state, true).expect("edited definition");
    assert_eq!(edited.controls[0].name, "Edited definition");
    assert_eq!(fixture.requests.borrow().len(), 7);
    *fixture.definition_name.borrow_mut() = "After reconnect".to_owned();
    assert_eq!(
        reconcile(&mut state, true).expect("reconnected").name,
        "After reconnect"
    );
    assert_eq!(fixture.requests.borrow().len(), 9);

    fixture.scene.borrow_mut()["zones"] = json!([]);
    assert!(reconcile(&mut state, false).is_none());
    assert!(
        state.begin(false).1.is_none(),
        "stopped effect retires its definition"
    );
}

fn snapshot(name: &str) -> PrimaryEffectView {
    PrimaryEffectView {
        id: EFFECT.to_owned(),
        zone_id: ZONE.to_owned(),
        layer_id: LAYER.to_owned(),
        name: name.to_owned(),
        controls: Vec::new(),
        control_values: HashMap::new(),
        active_preset_id: None,
        scene_revision: 1,
    }
}

#[test]
fn late_control_snapshot_cannot_overwrite_a_newer_full_refresh_or_its_metadata() {
    let mut state = PrimaryEffectRefresh::default();
    let first = state.begin(true).0;
    assert!(state.finish(first, Some(&snapshot("original"))));
    let old_control = state.begin(false).0;
    let (new_full, metadata) = state.begin(true);
    assert!(metadata.is_none());
    assert!(state.finish(new_full, Some(&snapshot("new definition"))));
    assert!(!state.finish(old_control, Some(&snapshot("stale"))));
    assert_eq!(
        state.begin(false).1.expect("current metadata").name,
        "new definition"
    );
}

#[test]
fn controls_during_a_pending_full_refresh_do_not_reuse_invalidated_definitions() {
    let mut state = PrimaryEffectRefresh::default();
    let first = state.begin(true).0;
    assert!(state.finish(first, Some(&snapshot("original"))));
    let old_full = state.begin(true).0;
    let (new_control, metadata) = state.begin(false);
    assert!(
        metadata.is_none(),
        "the newer request must fetch the definition itself"
    );
    assert!(!state.finish(old_full, Some(&snapshot("outdated full response"))));
    assert!(state.finish(new_control, Some(&snapshot("current"))));
    assert_eq!(
        state.begin(false).1.expect("current metadata").name,
        "current"
    );
}

#[test]
fn a_newer_empty_snapshot_prevents_old_responses_from_restoring_a_stopped_effect() {
    let mut state = PrimaryEffectRefresh::default();
    let old = state.begin(true).0;
    let newer = state.begin(false).0;
    assert!(state.finish(newer, None));
    assert!(!state.finish(old, Some(&snapshot("stopped"))));
    assert!(state.begin(false).1.is_none());
}
