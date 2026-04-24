//! Effect thumbnail capture and storage.
//!
//! Thumbnails are captured opportunistically from the live canvas frame
//! stream whenever an effect has been playing stably for a short window.
//! Each thumbnail stores a WebP data URL alongside a harmonized palette
//! extracted from the captured frame, keyed by effect ID + version in
//! localStorage. The UI reads from this store to paint effect cards with
//! their own screenshots and coordinated accent colors — no daemon work,
//! no build-time assets, no bandwidth explosion.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gloo_net::http::{Method, RequestBuilder};
use hypercolor_leptos_ext::canvas::{context_2d, create_canvas, image_data_rgba, set_canvas_size};
use hypercolor_leptos_ext::prelude::spawn_timeout;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;

use crate::color::{self, CanvasPalette};
use crate::ws::{CanvasFrame, CanvasPixelFormat};

const LOCAL_STORAGE_KEY: &str = "hypercolor:thumbnails";
const WEBP_QUALITY: f64 = 0.88;
/// How long an effect must play uninterrupted before we capture a thumbnail.
pub const CAPTURE_STABLE_MS: f64 = 2500.0;
/// Probe result cache for curated screenshots. Skips opportunistic capture
/// when the daemon already serves authored artwork for this slug, so we don't
/// burn localStorage quota on thumbnails that will be painted over anyway.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CuratedProbe {
    Pending,
    Present,
    Absent,
}

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
        let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else {
            return;
        };
        if let Err(err) = storage.set_item(LOCAL_STORAGE_KEY, &json) {
            // The entire thumbnail cache is persisted as one JSON blob, so a
            // quota trip loses the whole set, not just this one insert. Log
            // loudly so we notice when the localStorage ceiling starts biting
            // (typical browser quota is ~5 MB for the origin).
            web_sys::console::warn_2(
                &JsValue::from_str(&format!(
                    "thumbnail persist failed: cache_size_bytes={} entries={}",
                    json.len(),
                    cache.effects.len()
                )),
                &err,
            );
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
    if frame.pixel_format() == CanvasPixelFormat::Jpeg {
        return None;
    }

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
    let canvas = create_canvas()?;
    set_canvas_size(&canvas, frame.width, frame.height);

    let ctx = context_2d(&canvas).ok_or_else(|| JsValue::from_str("no 2d context"))?;

    let rgba = frame_to_rgba_vec(frame);
    let image_data = image_data_rgba(&rgba, frame.width, frame.height)?;
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

/// Kebab-case slug for an effect name. Mirrors the capture tool's slugify so
/// the UI and `effects/screenshots/curated/<slug>/` stay aligned.
fn slugify(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut prev_dash = true;
    for ch in value.chars() {
        let mapped = ch.to_ascii_lowercase();
        if mapped.is_ascii_alphanumeric() {
            out.push(mapped);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    out
}

fn curated_screenshot_url(slug: &str) -> String {
    format!("/api/v1/effects/screenshots/{slug}/default.webp")
}

/// Kick off a HEAD probe for a curated screenshot and update `probe_cache`
/// with the result. Idempotent — callers should check for an existing entry
/// before spawning.
fn spawn_curated_probe(slug: String, probe_cache: StoredValue<HashMap<String, CuratedProbe>>) {
    let url = curated_screenshot_url(&slug);
    wasm_bindgen_futures::spawn_local(async move {
        let request = match RequestBuilder::new(&url).method(Method::HEAD).build() {
            Ok(req) => req,
            Err(_) => {
                probe_cache.update_value(|cache| {
                    cache.insert(slug, CuratedProbe::Absent);
                });
                return;
            }
        };
        let state = match request.send().await {
            Ok(response) if response.status() == 200 => CuratedProbe::Present,
            _ => CuratedProbe::Absent,
        };
        probe_cache.update_value(|cache| {
            cache.insert(slug, state);
        });
    });
}

/// Background auto-capture loop.
///
/// Watches the active effect + canvas frame stream. When a single effect
/// has been playing for at least `CAPTURE_STABLE_MS` and no valid thumbnail
/// exists for the current version, captures one frame and stores it.
///
/// The `effect_lookup` closure maps an effect ID to `(slug, version)`. The
/// slug is used to probe the daemon's curated screenshot endpoint; when a
/// curated image exists we skip opportunistic capture entirely, since the
/// effect card will paint the authored artwork on top anyway.
pub fn install_auto_capture<F>(
    store: ThumbnailStore,
    active_effect_id: ReadSignal<Option<String>>,
    canvas_frame: ReadSignal<Option<CanvasFrame>>,
    effect_lookup: F,
) where
    F: Fn(&str) -> Option<(String, String)> + 'static,
{
    // Track when the current effect became active, so we can wait for the
    // stability window before capturing. Stored as (id, since_ms).
    let active_since: StoredValue<Option<(String, f64)>> = StoredValue::new(None);
    let pending_captures: StoredValue<HashSet<(String, String)>> = StoredValue::new(HashSet::new());
    let curated_probes: StoredValue<HashMap<String, CuratedProbe>> =
        StoredValue::new(HashMap::new());

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
        let Some((name, version)) = effect_lookup(&effect_id) else {
            return;
        };
        if store.get(&effect_id, &version).is_some() {
            return;
        }
        if pending_captures
            .with_value(|pending| pending.contains(&(effect_id.clone(), version.clone())))
        {
            return;
        }

        let slug = slugify(&name);
        let probe_state = curated_probes.with_value(|cache| cache.get(&slug).copied());
        match probe_state {
            Some(CuratedProbe::Present) => return,
            Some(CuratedProbe::Pending) => return,
            Some(CuratedProbe::Absent) => {}
            None => {
                curated_probes.update_value(|cache| {
                    cache.insert(slug.clone(), CuratedProbe::Pending);
                });
                spawn_curated_probe(slug, curated_probes);
                return;
            }
        }

        // Defer the actual capture to an idle callback so we don't block
        // the frame dispatch. This gives the effect a beat to render into
        // a representative state before we snapshot.
        let store = store;
        let effect_id_for_capture = effect_id.clone();
        let version_for_capture = version.clone();
        let frame_for_capture = frame.clone();
        let pending_key = (effect_id.clone(), version.clone());
        pending_captures.update_value(|pending| {
            pending.insert(pending_key.clone());
        });
        let pending_captures_for_callback = pending_captures;
        spawn_timeout(Duration::ZERO, move || {
            if let Some(thumbnail) = capture_thumbnail(&frame_for_capture, version_for_capture) {
                store.insert(effect_id_for_capture, thumbnail);
            }
            pending_captures_for_callback.update_value(|pending| {
                pending.remove(&pending_key);
            });
        });
    });
}

#[cfg(test)]
mod slug_tests {
    use super::slugify;

    #[test]
    fn slugify_converts_effect_names() {
        assert_eq!(slugify("Color Wave"), "color-wave");
        assert_eq!(slugify("ADHD Hyperfocus"), "adhd-hyperfocus");
        assert_eq!(slugify("  Spaced  Out  "), "spaced-out");
        assert_eq!(slugify("Nyan Dash!"), "nyan-dash");
    }
}
