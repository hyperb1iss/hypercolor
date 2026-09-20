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
