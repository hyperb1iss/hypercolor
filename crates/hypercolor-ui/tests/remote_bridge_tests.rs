#![cfg(target_arch = "wasm32")]

use hypercolor_ui::remote_bridge;
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(inline_js = r#"
export function installBridge(mode) {
  window.__hypercolorRemoteFatal = null;
  window.__HYPERCOLOR_REMOTE__ = {
    contract: mode === 'zero' ? {min: 0, max: 1}
      : mode === 'missing-request' ? {min: 1, max: 1}
      : {min: 2, max: 3},
    daemonId: "018f4c36-4a44-7cc9-9f57-0d2e9224d2f1",
    mount: "/remote/018f4c36-4a44-7cc9-9f57-0d2e9224d2f1",
    request() {}, openSocket() {}, ready() {},
    fatal(code) { window.__hypercolorRemoteFatal = code; },
  };
  if (mode === 'missing-request') delete window.__HYPERCOLOR_REMOTE__.request;
}

export function installWorkingBridge() {
  window.__hypercolorRemoteFatal = null;
  window.__HYPERCOLOR_REMOTE__ = {
    contract: {min: 1, max: 1},
    daemonId: "018f4c36-4a44-7cc9-9f57-0d2e9224d2f1",
    mount: "/remote/018f4c36-4a44-7cc9-9f57-0d2e9224d2f1",
    request() {}, openSocket() {}, ready() {},
    fatal(code) { window.__hypercolorRemoteFatal = code; },
    transportPath() { return {kind: "direct", generation: 3}; },
  };
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
    install_bridge("missing-request");
    assert!(remote_bridge::initialize().is_err());
    assert_eq!(recorded_fatal().as_deref(), Some("remote_contract_invalid"));
    clear_bridge();
}

#[wasm_bindgen_test]
fn bridged_app_keeps_its_asset_base_and_reads_the_transport_path() {
    install_working_bridge();
    let remote = remote_bridge::initialize()
        .expect("valid bridge")
        .expect("bridge present");
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
