use std::time::Duration;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

pub struct TimeoutHandle {
    id: Option<i32>,
    _callback: Option<JsValue>,
}

impl TimeoutHandle {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.id.is_some()
    }

    pub fn cancel(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        if let Some(window) = web_sys::window() {
            window.clear_timeout_with_handle(id);
        }
        self._callback = None;
    }
}

impl Drop for TimeoutHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub struct IntervalHandle {
    id: Option<i32>,
    _callback: Option<Closure<dyn FnMut()>>,
}

impl IntervalHandle {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.id.is_some()
    }

    pub fn cancel(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        if let Some(window) = web_sys::window() {
            window.clear_interval_with_handle(id);
        }
        self._callback = None;
    }
}

impl Drop for IntervalHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[must_use]
pub fn set_timeout<F>(delay: Duration, callback: F) -> TimeoutHandle
where
    F: FnOnce() + 'static,
{
    let callback = Closure::once_into_js(callback);
    let id = web_sys::window().and_then(|window| {
        window
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                callback.unchecked_ref(),
                duration_to_ms(delay),
            )
            .ok()
    });

    TimeoutHandle {
        id,
        _callback: Some(callback),
    }
}

#[must_use]
pub fn set_interval<F>(interval: Duration, callback: F) -> IntervalHandle
where
    F: FnMut() + 'static,
{
    let callback = Closure::<dyn FnMut()>::new(callback);
    let id = web_sys::window().and_then(|window| {
        window
            .set_interval_with_callback_and_timeout_and_arguments_0(
                callback.as_ref().unchecked_ref(),
                duration_to_ms(interval),
            )
            .ok()
    });

    IntervalHandle {
        id,
        _callback: Some(callback),
    }
}

pub fn spawn_timeout<F>(delay: Duration, callback: F) -> bool
where
    F: FnOnce() + 'static,
{
    let Some(window) = web_sys::window() else {
        callback();
        return false;
    };

    let callback = Closure::once(callback);
    let Ok(_id) = window.set_timeout_with_callback_and_timeout_and_arguments_0(
        callback.as_ref().unchecked_ref(),
        duration_to_ms(delay),
    ) else {
        return false;
    };

    callback.forget();
    true
}

pub async fn sleep(delay: Duration) {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let Some(window) = web_sys::window() else {
            let _ = resolve.call0(&JsValue::UNDEFINED);
            return;
        };

        if window
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, duration_to_ms(delay))
            .is_err()
        {
            let _ = resolve.call0(&JsValue::UNDEFINED);
        }
    });

    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

fn duration_to_ms(duration: Duration) -> i32 {
    i32::try_from(duration.as_millis())
        .unwrap_or(i32::MAX)
        .max(0)
}
