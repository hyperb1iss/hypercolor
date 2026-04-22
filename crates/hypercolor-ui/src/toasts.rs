//! Toast notification helpers — thin wrappers around leptoaster.

use leptoaster::{ToastBuilder, ToastLevel, ToasterContext};
use leptos::prelude::{GetUntracked, Set, use_context};
use wasm_bindgen::{JsCast, closure::Closure};

const TOAST_EXPIRY_MS: u32 = 2_500;
const TOAST_EXIT_MS: i32 = 200;

pub fn toast_success(msg: &str) {
    toast(msg, ToastLevel::Success);
}

pub fn toast_error(msg: &str) {
    toast(msg, ToastLevel::Error);
}

pub fn toast_info(msg: &str) {
    toast(msg, ToastLevel::Info);
}

fn toast(msg: &str, level: ToastLevel) {
    let Some(toaster) = use_context::<ToasterContext>() else {
        log::warn!("toast without ToasterContext: {msg}");
        return;
    };

    toaster.toast(
        ToastBuilder::new(msg)
            .with_level(level)
            .with_expiry(Some(TOAST_EXPIRY_MS)),
    );

    let Some(toast) = toaster.queue.get_untracked().last().cloned() else {
        return;
    };

    schedule_timeout(TOAST_EXPIRY_MS as i32, move || {
        toast.clear_signal.set(true);

        let toaster = toaster.clone();
        schedule_timeout(TOAST_EXIT_MS, move || {
            toaster.remove(toast.id);
        });
    });
}

fn schedule_timeout(delay_ms: i32, callback: impl FnOnce() + 'static) {
    let Some(window) = web_sys::window() else {
        callback();
        return;
    };

    let callback = Closure::once(callback);
    if let Err(error) = window.set_timeout_with_callback_and_timeout_and_arguments_0(
        callback.as_ref().unchecked_ref(),
        delay_ms,
    ) {
        log::warn!("failed to schedule toast timeout: {error:?}");
        return;
    }
    callback.forget();
}
