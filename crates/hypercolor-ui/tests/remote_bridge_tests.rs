#![cfg(target_arch = "wasm32")]

use hypercolor_ui::remote_bridge;
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(inline_js = r#"
export function installIncompatibleBridge() {
  window.__hypercolorRemoteFatal = null;
  window.__HYPERCOLOR_REMOTE__ = {
    contract: {min: 2, max: 3},
    daemonId: "018f4c36-4a44-7cc9-9f57-0d2e9224d2f1",
    mount: "/remote/018f4c36-4a44-7cc9-9f57-0d2e9224d2f1",
    fatal(code) { window.__hypercolorRemoteFatal = code; },
  };
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
    #[wasm_bindgen(js_name = installIncompatibleBridge)]
    fn install_incompatible_bridge();
    #[wasm_bindgen(js_name = recordedFatal)]
    fn recorded_fatal() -> Option<String>;
    #[wasm_bindgen(js_name = clearBridge)]
    fn clear_bridge();
}

#[wasm_bindgen_test]
fn incompatible_contract_calls_fatal_and_refuses_startup() {
    install_incompatible_bridge();
    assert!(remote_bridge::initialize().is_err());
    assert_eq!(recorded_fatal().as_deref(), Some("remote_contract_mismatch"));
    clear_bridge();
}
