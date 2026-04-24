#![cfg(target_arch = "wasm32")]

use hypercolor_leptos_ext::canvas::{
    context_2d, create_canvas, image_data_rgba, revoke_blob_url, script_blob_url, set_canvas_size,
};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn canvas_2d_accepts_image_data() {
    let canvas = create_canvas().expect("canvas is created");
    assert!(set_canvas_size(&canvas, 1, 1));
    let context = context_2d(&canvas).expect("2d context is available");
    let pixel = [255, 0, 128, 255];
    let image_data = image_data_rgba(&pixel, 1, 1).expect("image data is created");
    context
        .put_image_data(&image_data, 0.0, 0.0)
        .expect("image data is written");
}

#[wasm_bindgen_test]
fn script_blob_url_can_be_revoked() {
    let url = script_blob_url("self.onmessage = () => {};").expect("script blob URL is created");
    assert!(url.starts_with("blob:"));
    assert!(revoke_blob_url(&url));
}
