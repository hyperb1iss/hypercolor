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

#[must_use]
pub fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map_or_else(js_sys::Date::now, |performance| performance.now())
}

#[must_use]
pub fn random_unit() -> f64 {
    js_sys::Math::random()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageLocation {
    pub protocol: String,
    pub hostname: String,
    pub port: String,
}

impl PageLocation {
    #[must_use]
    pub fn websocket_protocol(&self) -> &'static str {
        if self.protocol == "https:" {
            "wss:"
        } else {
            "ws:"
        }
    }

    #[must_use]
    pub fn host(&self) -> String {
        if self.port.is_empty() {
            self.hostname.clone()
        } else {
            format!("{}:{}", self.hostname, self.port)
        }
    }
}

#[must_use]
pub fn current_page_location() -> PageLocation {
    let Some(location) = web_sys::window().map(|window| window.location()) else {
        return PageLocation {
            protocol: "http:".to_string(),
            hostname: "127.0.0.1".to_string(),
            port: String::new(),
        };
    };

    PageLocation {
        protocol: location.protocol().unwrap_or_else(|_| "http:".to_string()),
        hostname: location
            .hostname()
            .unwrap_or_else(|_| "127.0.0.1".to_string()),
        port: location.port().unwrap_or_default(),
    }
}

#[must_use]
pub fn viewport_width(default: f64) -> f64 {
    web_sys::window()
        .and_then(|window| window.inner_width().ok())
        .and_then(|value| value.as_f64())
        .unwrap_or(default)
}

#[must_use]
pub fn viewport_height(default: f64) -> f64 {
    web_sys::window()
        .and_then(|window| window.inner_height().ok())
        .and_then(|value| value.as_f64())
        .unwrap_or(default)
}

#[must_use]
pub fn device_pixel_ratio(default: f64) -> f64 {
    web_sys::window().map_or(default, |window| window.device_pixel_ratio())
}

pub fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|window| window.local_storage().ok().flatten())
}

pub fn storage_get(key: &str) -> Option<String> {
    local_storage()?.get_item(key).ok().flatten()
}

pub fn storage_get_parsed<T: std::str::FromStr>(key: &str) -> Option<T> {
    storage_get(key)?.parse().ok()
}

pub fn storage_get_clamped(key: &str, default: f64, min: f64, max: f64) -> f64 {
    storage_get_parsed::<f64>(key)
        .map(|value| value.clamp(min, max))
        .unwrap_or(default)
}

pub fn storage_set(key: &str, value: &str) -> bool {
    storage_try_set(key, value).is_ok()
}

pub fn storage_try_set(key: &str, value: &str) -> Result<(), JsValue> {
    let Some(storage) = local_storage() else {
        return Err(JsValue::from_str("localStorage unavailable"));
    };
    storage.set_item(key, value)
}

pub fn storage_remove(key: &str) -> bool {
    local_storage().is_some_and(|storage| storage.remove_item(key).is_ok())
}

fn duration_to_ms(duration: Duration) -> i32 {
    i32::try_from(duration.as_millis())
        .unwrap_or(i32::MAX)
        .max(0)
}
