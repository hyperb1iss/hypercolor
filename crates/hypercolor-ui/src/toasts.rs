//! Toast notification helpers — thin wrappers around leptoaster.

use std::cell::RefCell;
use std::time::Duration;

use hypercolor_leptos_ext::prelude::spawn_timeout;
use leptoaster::{ToastBuilder, ToastLevel, ToasterContext};
use leptos::prelude::{GetUntracked, Set, use_context};

const TOAST_EXPIRY_MS: u32 = 2_500;
const TOAST_EXIT_MS: u64 = 200;

thread_local! {
    static ROOT_TOASTER: RefCell<Option<ToasterContext>> = const { RefCell::new(None) };
}

pub fn install_root_context() {
    let Some(toaster) = use_context::<ToasterContext>() else {
        log::warn!("cannot install root toaster without ToasterContext");
        return;
    };
    ROOT_TOASTER.with(|root| root.replace(Some(toaster)));
}

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
    let Some(toaster) =
        use_context::<ToasterContext>().or_else(|| ROOT_TOASTER.with(|root| root.borrow().clone()))
    else {
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

    schedule_timeout(u64::from(TOAST_EXPIRY_MS), move || {
        toast.clear_signal.set(true);

        let toaster = toaster.clone();
        schedule_timeout(TOAST_EXIT_MS, move || {
            toaster.remove(toast.id);
        });
    });
}

fn schedule_timeout(delay_ms: u64, callback: impl FnOnce() + 'static) {
    if !spawn_timeout(Duration::from_millis(delay_ms), callback) {
        log::warn!("failed to schedule toast timeout");
    }
}
