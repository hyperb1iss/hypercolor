//! Fire-and-forget async helpers for common API call patterns.
//!
//! Covers the recurring call shapes that appear throughout the UI:
//! - `spawn_mutation` — the composable primitive: run a fallible future,
//!   route the result to an `on_ok`/`on_err` pair.
//! - `toast_on_err` / `with_rollback` — building blocks for the common
//!   error arms (toast a prefixed message; undo an optimistic update
//!   before handling the error).
//! - `spawn_api_call` — fire and forget; toast any error.
//! - `spawn_identify` — fire and forget; toast "Flashing …" on success or
//!   "Identify failed: …" on error.

/// Spawn a mutation future, routing `Ok` to `on_ok` and `Err` to `on_err`.
///
/// The composable core of the `spawn_local + match + toast_error` shape:
/// pair it with [`toast_on_err`] for the standard error toast, and wrap
/// the error arm in [`with_rollback`] when an optimistic update needs to
/// be undone first.
pub fn spawn_mutation<T: 'static>(
    fut: impl std::future::Future<Output = Result<T, String>> + 'static,
    on_ok: impl FnOnce(T) + 'static,
    on_err: impl FnOnce(String) + 'static,
) {
    leptos::task::spawn_local(async move {
        match fut.await {
            Ok(value) => on_ok(value),
            Err(error) => on_err(error),
        }
    });
}

/// Build an error arm for [`spawn_mutation`] that toasts
/// `"{prefix}: {error}"`.
pub fn toast_on_err(prefix: &'static str) -> impl FnOnce(String) + 'static {
    move |error| crate::toasts::toast_error(&format!("{prefix}: {error}"))
}

/// Wrap an error arm so `undo` runs first — the rollback-on-failure shape
/// for optimistic updates. `undo` restores the captured pre-mutation
/// state, then the error is handed to `on_err` (typically
/// [`toast_on_err`]).
pub fn with_rollback(
    undo: impl FnOnce() + 'static,
    on_err: impl FnOnce(String) + 'static,
) -> impl FnOnce(String) + 'static {
    move |error| {
        undo();
        on_err(error);
    }
}

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
