#![cfg(target_arch = "wasm32")]

use hypercolor_ui::remote_bridge;
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(inline_js = r#"
const DAEMON = "018f4c36-4a44-7cc9-9f57-0d2e9224d2f1";

function bridge(min, max, preview) {
  const value = {
    contract: {min, max},
    daemonId: DAEMON,
    mount: `/remote/${DAEMON}`,
    request() {}, openSocket() {}, ready() {},
    fatal(code) { window.__hypercolorRemoteFatal = code; },
  };
  if (preview) {
    value.previewState = () => "connecting";
    value.previewStream = () => null;
    value.setPreview = () => {};
  }
  return value;
}

export function bridgeObject(min, max, preview) {
  return bridge(min, max, preview);
}

export function installBridge(mode) {
  window.__hypercolorRemoteFatal = null;
  window.__HYPERCOLOR_REMOTE__ =
    mode === 'zero' ? bridge(0, 1, false)
    : mode === 'mismatch' ? bridge(3, 4, true)
    : mode === 'missing-preview' ? bridge(2, 2, false)
    : bridge(1, 1, false);
  if (mode === 'missing-request') delete window.__HYPERCOLOR_REMOTE__.request;
}

export function installWorkingBridge() {
  window.__hypercolorRemoteFatal = null;
  window.__HYPERCOLOR_REMOTE__ = bridge(1, 1, false);
  window.__HYPERCOLOR_REMOTE__.transportPath = () => ({kind: "direct", generation: 3});
}

export function pathEvent(detail) {
  return new CustomEvent("hypercolor:remote-transport-path", {detail});
}

export function recordedFatal() {
  return window.__hypercolorRemoteFatal;
}

export function clearBridge() {
  delete window.__HYPERCOLOR_REMOTE__;
  delete window.__hypercolorRemoteFatal;
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = bridgeObject)]
    fn bridge_object(min: u32, max: u32, preview: bool) -> JsValue;
    #[wasm_bindgen(js_name = installBridge)]
    fn install_bridge(mode: &str);
    #[wasm_bindgen(js_name = installWorkingBridge)]
    fn install_working_bridge();
    #[wasm_bindgen(js_name = pathEvent)]
    fn path_event(detail: &JsValue) -> web_sys::Event;
    #[wasm_bindgen(js_name = recordedFatal)]
    fn recorded_fatal() -> Option<String>;
    #[wasm_bindgen(js_name = clearBridge)]
    fn clear_bridge();
}

#[wasm_bindgen_test]
fn incompatible_contract_calls_fatal_and_refuses_startup() {
    for mode in ["mismatch", "zero"] {
        install_bridge(mode);
        assert!(remote_bridge::initialize().is_err());
        assert_eq!(
            recorded_fatal().as_deref(),
            Some("remote_contract_mismatch")
        );
    }
    for mode in ["missing-request", "missing-preview"] {
        install_bridge(mode);
        assert!(remote_bridge::initialize().is_err());
        assert_eq!(
            recorded_fatal().as_deref(),
            Some("remote_contract_invalid"),
            "{mode}"
        );
    }
    clear_bridge();
}

#[wasm_bindgen_test]
fn contract_one_bridges_negotiate_without_video() {
    for preview in [false, true] {
        let negotiated =
            remote_bridge::negotiate(&bridge_object(1, 1, preview)).expect("contract 1 bridge");
        assert_eq!(negotiated.contract, 1);
        assert!(negotiated.preview.is_none(), "preview members: {preview}");
    }
}

#[wasm_bindgen_test]
fn contract_two_bridges_negotiate_the_video_preview() {
    for (min, max) in [(1, 2), (2, 2), (1, 5), (2, 9)] {
        let negotiated =
            remote_bridge::negotiate(&bridge_object(min, max, true)).expect("contract 2 bridge");
        assert_eq!(negotiated.contract, 2, "{min}..={max}");
        let preview = negotiated.preview.expect("contract 2 carries the preview");
        assert_eq!(preview.state(), remote_bridge::PreviewState::Connecting);
        assert!(preview.stream().is_none());
    }
}

#[wasm_bindgen_test]
fn a_contract_two_bridge_must_carry_its_preview_members() {
    for (min, max) in [(1, 2), (2, 2)] {
        let refused = remote_bridge::negotiate(&bridge_object(min, max, false))
            .err()
            .expect("preview members are required");
        assert_eq!(refused.code, "remote_contract_invalid");
    }
    let refused = remote_bridge::negotiate(&bridge_object(3, 4, true))
        .err()
        .expect("contract 3 is beyond this build");
    assert_eq!(refused.code, "remote_contract_mismatch");
}

#[wasm_bindgen_test]
fn bridged_app_keeps_its_asset_base_and_reads_the_transport_path() {
    install_working_bridge();
    let remote = remote_bridge::initialize()
        .expect("valid bridge")
        .expect("bridge present");
    assert_eq!(remote.contract(), 1);
    assert!(remote_bridge::preview_channel().is_none());
    let deployed = hypercolor_ui::UiMount::new("", "/remote-app/versions/0.6.0-beta.7")
        .expect("valid asset mount");
    let mount = remote.ui_mount(&deployed);
    assert_eq!(
        mount.route_href("/devices"),
        "/remote/018f4c36-4a44-7cc9-9f57-0d2e9224d2f1/devices"
    );
    assert_eq!(
        mount.asset_href("/assets/brand/mark-color.png"),
        "/remote-app/versions/0.6.0-beta.7/assets/brand/mark-color.png"
    );

    let report = remote_bridge::current_transport_path().expect("bridge reports a path");
    assert_eq!((report.kind.as_str(), report.generation), ("direct", 3));

    let detail = js_sys::JSON::parse(r#"{"kind":"relay","generation":4}"#).expect("valid json");
    let report = remote_bridge::transport_path_from_event(&path_event(&detail))
        .expect("event carries a path");
    assert_eq!((report.kind.as_str(), report.generation), ("relay", 4));
    assert!(remote_bridge::transport_path_from_event(&path_event(&JsValue::NULL)).is_none());
    clear_bridge();
}
