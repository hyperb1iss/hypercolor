//! Effect thumbnail capture and storage.
//!
//! Thumbnails are captured opportunistically from the live canvas frame
//! stream whenever an effect has been playing stably for a short window.
//! Each thumbnail stores a WebP data URL alongside a harmonized palette
//! extracted from the captured frame, keyed by effect ID + version in
//! localStorage. The UI reads from this store to paint effect cards with
//! their own screenshots and coordinated accent colors — no daemon work,
//! no build-time assets, no bandwidth explosion.

use std::collections::HashMap;

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::color::{self, CanvasPalette};
use crate::ws::CanvasFrame;

const LOCAL_STORAGE_KEY: &str = "hypercolor:thumbnails";
const WEBP_QUALITY: f64 = 0.7;
/// How long an effect must play uninterrupted before we capture a thumbnail.
pub const CAPTURE_STABLE_MS: f64 = 2500.0;

/// Serialized palette — RGB strings ready for direct use in CSS `rgb(...)`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ThumbnailPalette {
    pub primary: String,
    pub secondary: String,
    pub tertiary: String,
}

impl ThumbnailPalette {
    /// Build a harmonized thumbnail palette from a raw canvas palette.
    fn from_canvas(raw: CanvasPalette) -> Self {
        let harmonized = color::harmonize_palette(raw);
        Self {
            primary: color::rgb_string(harmonized.primary),
            secondary: color::rgb_string(harmonized.secondary),
            tertiary: color::rgb_string(harmonized.tertiary),
        }
    }
}

/// One captured thumbnail — compressed image + palette + metadata.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Thumbnail {
    /// `data:image/webp;base64,...` suitable for direct use in CSS `url(...)`.
    pub data_url: String,
    pub palette: ThumbnailPalette,
    /// Effect version at capture time — used to invalidate stale thumbnails.
    pub version: String,
    /// Epoch milliseconds when the capture was taken.
    pub captured_at: f64,
}

/// Full thumbnail cache, persisted to localStorage as a single JSON blob.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct ThumbnailCache {
    #[serde(default)]
    effects: HashMap<String, Thumbnail>,
}

/// Reactive thumbnail store — read from components, write from the capture loop.
#[derive(Clone, Copy)]
pub struct ThumbnailStore {
    store: RwSignal<HashMap<String, Thumbnail>>,
}

impl ThumbnailStore {
    pub fn new() -> Self {
        let initial = load_from_storage().unwrap_or_default();
        Self {
            store: RwSignal::new(initial.effects),
        }
    }

    /// Returns the current thumbnail for an effect if one exists and
    /// matches the given version. Reactive: components re-render when
    /// the store updates.
    pub fn get(&self, effect_id: &str, version: &str) -> Option<Thumbnail> {
        self.store.with(|map| {
            map.get(effect_id)
                .and_then(|thumb| (thumb.version == version).then(|| thumb.clone()))
        })
    }

    /// Insert or replace a thumbnail and persist the full store to localStorage.
    pub fn insert(&self, effect_id: String, thumbnail: Thumbnail) {
        self.store.update(|map| {
            map.insert(effect_id, thumbnail);
        });
        self.persist();
    }

    fn persist(&self) {
        let cache = ThumbnailCache {
            effects: self.store.get_untracked(),
        };
        let Ok(json) = serde_json::to_string(&cache) else {
            return;
        };
        if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            let _ = storage.set_item(LOCAL_STORAGE_KEY, &json);
        }
    }
}

impl Default for ThumbnailStore {
    fn default() -> Self {
        Self::new()
    }
}

fn load_from_storage() -> Option<ThumbnailCache> {
    let storage = web_sys::window().and_then(|w| w.local_storage().ok().flatten())?;
    let raw = storage.get_item(LOCAL_STORAGE_KEY).ok().flatten()?;
    serde_json::from_str(&raw).ok()
}

/// Capture a thumbnail from a canvas frame synchronously.
///
/// Builds an ImageData on an offscreen HtmlCanvasElement and encodes it
/// to a WebP data URL. Returns `None` if palette extraction fails (blank
/// frame) or any DOM operation errors out — both are treated as "skip this
/// capture, try again later" rather than fatal.
pub fn capture_thumbnail(frame: &CanvasFrame, version: String) -> Option<Thumbnail> {
    let palette = color::extract_canvas_palette(frame)?;
    let data_url = encode_frame_to_webp(frame).ok()?;

    Some(Thumbnail {
        data_url,
        palette: ThumbnailPalette::from_canvas(palette),
        version,
        captured_at: js_sys::Date::now(),
    })
}

fn encode_frame_to_webp(frame: &CanvasFrame) -> Result<String, JsValue> {
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let canvas: web_sys::HtmlCanvasElement = document
        .create_element("canvas")?
        .dyn_into()
        .map_err(|_| JsValue::from_str("not an HtmlCanvasElement"))?;

    canvas.set_width(frame.width);
    canvas.set_height(frame.height);

    let ctx: web_sys::CanvasRenderingContext2d = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("no 2d context"))?
        .dyn_into()
        .map_err(|_| JsValue::from_str("not a 2d context"))?;

    let rgba = frame_to_rgba_vec(frame);
    let clamped = wasm_bindgen::Clamped(rgba.as_slice());
    let image_data =
        web_sys::ImageData::new_with_u8_clamped_array_and_sh(clamped, frame.width, frame.height)?;
    ctx.put_image_data(&image_data, 0.0, 0.0)?;

    canvas.to_data_url_with_type_and_encoder_options("image/webp", &JsValue::from_f64(WEBP_QUALITY))
}

fn frame_to_rgba_vec(frame: &CanvasFrame) -> Vec<u8> {
    let pixel_count = frame.pixel_count();
    let mut rgba = Vec::with_capacity(pixel_count.saturating_mul(4));
    for i in 0..pixel_count {
        if let Some([r, g, b, a]) = frame.rgba_at(i) {
            rgba.extend_from_slice(&[r, g, b, a]);
        } else {
            rgba.extend_from_slice(&[0, 0, 0, 255]);
        }
    }
    rgba
}

/// Background auto-capture loop.
///
/// Watches the active effect + canvas frame stream. When a single effect
/// has been playing for at least `CAPTURE_STABLE_MS` and no valid thumbnail
/// exists for the current version, captures one frame and stores it.
///
/// The `version_lookup` closure should return the effect's current version
/// string — this is used to invalidate stale thumbnails when an effect
/// changes.
pub fn install_auto_capture<F>(
    store: ThumbnailStore,
    active_effect_id: ReadSignal<Option<String>>,
    canvas_frame: ReadSignal<Option<CanvasFrame>>,
    version_lookup: F,
) where
    F: Fn(&str) -> Option<String> + 'static,
{
    // Track when the current effect became active, so we can wait for the
    // stability window before capturing. Stored as (id, since_ms).
    let active_since: StoredValue<Option<(String, f64)>> = StoredValue::new(None);

    Effect::new(move |_| {
        // React to active effect changes — reset the stability timer.
        let current_id = active_effect_id.get();
        let now = js_sys::Date::now();

        active_since.update_value(|state| match (state.as_ref(), current_id.as_ref()) {
            (Some((prev, _)), Some(curr)) if prev == curr => {}
            (_, Some(curr)) => *state = Some((curr.clone(), now)),
            (_, None) => *state = None,
        });

        // Reactively watch canvas frames — the actual capture happens here.
        let Some(frame) = canvas_frame.get() else {
            return;
        };
        let Some((effect_id, started_at)) = active_since.get_value() else {
            return;
        };
        if now - started_at < CAPTURE_STABLE_MS {
            return;
        }

        // Check if we already have a fresh thumbnail for this version.
        let Some(version) = version_lookup(&effect_id) else {
            return;
        };
        if store.get(&effect_id, &version).is_some() {
            return;
        }

        // Defer the actual capture to an idle callback so we don't block
        // the frame dispatch. This gives the effect a beat to render into
        // a representative state before we snapshot.
        let store = store;
        let effect_id_for_capture = effect_id.clone();
        let version_for_capture = version.clone();
        let frame_for_capture = frame.clone();
        let cb = Closure::once_into_js(move || {
            if let Some(thumbnail) = capture_thumbnail(&frame_for_capture, version_for_capture) {
                store.insert(effect_id_for_capture, thumbnail);
            }
        });
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                0,
            );
        }
    });
}
