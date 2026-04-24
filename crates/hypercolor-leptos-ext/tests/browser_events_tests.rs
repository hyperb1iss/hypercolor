#![cfg(target_arch = "wasm32")]

use std::cell::Cell;
use std::rc::Rc;

use hypercolor_leptos_ext::events::{Input, on};
use wasm_bindgen::JsCast;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

fn document() -> web_sys::Document {
    web_sys::window()
        .and_then(|window| window.document())
        .expect("browser document is available")
}

#[wasm_bindgen_test]
fn event_handle_detaches_on_drop() {
    let document = document();
    let button = document
        .create_element("button")
        .expect("button element is created");
    document
        .body()
        .expect("document body is available")
        .append_child(&button)
        .expect("button is appended");

    let clicks = Rc::new(Cell::new(0));
    let clicks_handle = Rc::clone(&clicks);
    let handle = on(button.unchecked_ref(), "click", move |_| {
        clicks_handle.set(clicks_handle.get() + 1);
    });

    let click = web_sys::Event::new("click").expect("click event is created");
    button
        .dispatch_event(&click)
        .expect("first click is dispatched");
    drop(handle);
    button
        .dispatch_event(&click)
        .expect("second click is dispatched");

    assert_eq!(clicks.get(), 1);
    button.remove();
}

#[wasm_bindgen_test]
fn input_event_reads_string_value() {
    let document = document();
    let input = document
        .create_element("input")
        .expect("input element is created")
        .dyn_into::<web_sys::HtmlInputElement>()
        .expect("element is an input");
    input.set_value("42");
    document
        .body()
        .expect("document body is available")
        .append_child(&input)
        .expect("input is appended");

    let seen = Rc::new(Cell::new(false));
    let seen_handle = Rc::clone(&seen);
    let handle = on(input.unchecked_ref(), "input", move |event| {
        let event = Input::from_event(event.clone());
        seen_handle.set(event.value_string().as_deref() == Some("42"));
    });

    let event = web_sys::Event::new("input").expect("input event is created");
    input
        .dispatch_event(&event)
        .expect("input event is dispatched");

    assert!(seen.get());
    drop(handle);
    input.remove();
}
