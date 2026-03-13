//! Toast notification helpers — thin wrappers around leptoaster.

use leptoaster::ToasterContext;
use leptos::prelude::use_context;

pub fn toast_success(msg: &str) {
    if let Some(toaster) = use_context::<ToasterContext>() {
        toaster.success(msg);
    } else {
        log::warn!("toast_success without ToasterContext: {msg}");
    }
}

pub fn toast_error(msg: &str) {
    if let Some(toaster) = use_context::<ToasterContext>() {
        toaster.error(msg);
    } else {
        log::warn!("toast_error without ToasterContext: {msg}");
    }
}

pub fn toast_info(msg: &str) {
    if let Some(toaster) = use_context::<ToasterContext>() {
        toaster.info(msg);
    } else {
        log::warn!("toast_info without ToasterContext: {msg}");
    }
}
