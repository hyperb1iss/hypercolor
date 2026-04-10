//! Fire-and-forget async helpers for common API call patterns.
//!
//! Covers two recurring call shapes that appear throughout the UI:
//! - `spawn_api_call` — fire and forget; toast any error.
//! - `spawn_identify` — fire and forget; toast "Flashing …" on success or
//!   "Identify failed: …" on error.

/// Spawn an API call. If it returns an `Err`, shows `"{err_prefix}: {e}"`.
pub fn spawn_api_call<T, Fut>(err_prefix: &'static str, future: Fut)
where
    T: 'static,
    Fut: std::future::Future<Output = Result<T, String>> + 'static,
{
    leptos::task::spawn_local(async move {
        if let Err(e) = future.await {
            crate::toasts::toast_error(&format!("{err_prefix}: {e}"));
        }
    });
}

/// Spawn a flash/identify call.
///
/// On `Ok`, shows `"Flashing {what}"`. On `Err`, shows `"Identify failed: {e}"`.
pub fn spawn_identify<Fut>(what: &'static str, future: Fut)
where
    Fut: std::future::Future<Output = Result<(), String>> + 'static,
{
    leptos::task::spawn_local(async move {
        match future.await {
            Ok(()) => crate::toasts::toast_success(&format!("Flashing {what}")),
            Err(e) => crate::toasts::toast_error(&format!("Identify failed: {e}")),
        }
    });
}
