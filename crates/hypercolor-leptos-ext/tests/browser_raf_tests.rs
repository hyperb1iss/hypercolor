#![cfg(target_arch = "wasm32")]

use std::cell::Cell;
use std::rc::Rc;

use hypercolor_leptos_ext::raf::Scheduler;
use wasm_bindgen::JsCast;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test(async)]
async fn scheduler_runs_once_for_single_schedule() {
    let calls = Rc::new(Cell::new(0));
    let calls_handle = Rc::clone(&calls);
    let scheduler = Scheduler::new(move |_| {
        calls_handle.set(calls_handle.get() + 1);
    });

    scheduler.schedule();
    wait_for_animation_frame().await;
    wait_for_animation_frame().await;

    assert_eq!(calls.get(), 1);
    assert!(!scheduler.is_pending());
}

async fn wait_for_animation_frame() {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let callback = wasm_bindgen::closure::Closure::once(move || {
            let _ = resolve.call0(&wasm_bindgen::JsValue::UNDEFINED);
        });
        web_sys::window()
            .expect("browser window is available")
            .request_animation_frame(callback.as_ref().unchecked_ref())
            .expect("animation frame is scheduled");
        callback.forget();
    });

    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}
