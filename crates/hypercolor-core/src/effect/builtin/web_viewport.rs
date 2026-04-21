use std::net::IpAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::bail;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::effect::{
    ControlDefinition, ControlValue, EffectCategory, EffectMetadata, EffectSource, PreviewSource,
};
use hypercolor_types::viewport::{FitMode, ViewportRect};

use super::common::{
    builtin_effect_id, dropdown_control, rect_control, slider_control, text_control,
};
use crate::effect::servo::{ServoSessionHandle, SessionConfig, note_servo_session_error};
use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};
use crate::spatial::sample_viewport;

const URL_LOAD_DEBOUNCE: Duration = Duration::from_millis(250);
const BLANK_URL: &str = "about:blank";

pub struct WebViewportRenderer {
    session: Option<ServoSessionHandle>,
    url: String,
    viewport: ViewportRect,
    fit_mode: FitMode,
    brightness: f32,
    refresh_interval_secs: f32,
    render_width: u32,
    render_height: u32,
    scroll_x: i32,
    scroll_y: i32,
    /// Last `(scroll_x, scroll_y)` we dispatched to Servo. Lets us
    /// skip injecting the `window.scrollTo` script on frames where
    /// scroll hasn't changed — keeps the JS cost off the happy path
    /// and avoids redundant work for pages sitting at the top.
    last_applied_scroll: Option<(i32, i32)>,
    last_refresh_time_secs: Option<f32>,
    url_dirty_at: Option<Instant>,
    loaded_url: Option<String>,
    preview_canvas: Option<Canvas>,
    load_failed: bool,
}

/// Matches Spec 46 § 8.1 — both axes share the same 0..32768 upper
/// bound so the control definition and the `set_control` clamp stay
/// symmetric.
const SCROLL_AXIS_MAX: f32 = 32768.0;

impl WebViewportRenderer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            session: None,
            url: "https://example.com".to_owned(),
            viewport: ViewportRect::full(),
            fit_mode: FitMode::Cover,
            brightness: 1.0,
            refresh_interval_secs: 0.0,
            render_width: 1280,
            render_height: 720,
            scroll_x: 0,
            scroll_y: 0,
            last_applied_scroll: None,
            last_refresh_time_secs: None,
            url_dirty_at: None,
            loaded_url: None,
            preview_canvas: None,
            load_failed: false,
        }
    }

    /// Build the list of scripts to evaluate before Servo paints the
    /// next frame. Today that's just scroll positioning; future controls
    /// can add to this list without reshaping the call sites.
    ///
    /// Absolute scroll is the contract the spec promises the UI; Servo's
    /// embedder only exposes relative scroll deltas, so we drive it
    /// through `window.scrollTo({behavior: 'instant'})`. Skipping the
    /// script when scroll hasn't changed AND is at the origin avoids
    /// wasted JS execution on every paint for pages that never scroll.
    fn pending_scripts_for_render(&self) -> Vec<String> {
        // "Never dispatched" is semantically identical to "dispatched
        // (0, 0)" — Servo starts every fresh page at the origin, so
        // asking it to scrollTo(0, 0) right after load is redundant.
        // Only emit when the requested scroll differs from the
        // baseline we can prove Servo is already at.
        let current = (self.scroll_x, self.scroll_y);
        let baseline = self.last_applied_scroll.unwrap_or((0, 0));
        if current == baseline {
            return Vec::new();
        }
        vec![format!(
            "window.scrollTo({{left: {}, top: {}, behavior: 'instant'}});",
            self.scroll_x, self.scroll_y,
        )]
    }

    fn session_config(&self) -> SessionConfig {
        SessionConfig {
            render_width: self.render_width,
            render_height: self.render_height,
            inject_engine_globals: false,
        }
    }

    fn load_url_now(&mut self) -> anyhow::Result<()> {
        let Some(session) = self.session.as_mut() else {
            return Ok(());
        };
        let url = normalize_web_url_input(&self.url);
        if url.is_empty() {
            bail!("web viewport URL is empty");
        }
        match session.load_url(&url) {
            Ok(()) => {
                self.url.clone_from(&url);
                self.loaded_url = Some(url);
                self.load_failed = false;
                self.url_dirty_at = None;
                // Navigation resets Servo's scroll position. Force
                // our scroll to be re-applied on the next paint.
                self.last_applied_scroll = None;
                Ok(())
            }
            Err(error) => {
                self.loaded_url = None;
                self.preview_canvas = None;
                self.load_failed = true;
                self.url_dirty_at = None;
                let _ = session.load_url(BLANK_URL);
                Err(error)
            }
        }
    }

    fn maybe_load_pending_url(&mut self) -> anyhow::Result<()> {
        let should_load = self
            .url_dirty_at
            .is_some_and(|dirty_at| dirty_at.elapsed() >= URL_LOAD_DEBOUNCE)
            && self.loaded_url.as_deref() != Some(self.url.as_str());
        if should_load {
            self.load_url_now()?;
        }
        Ok(())
    }

    fn maybe_reload(&mut self, time_secs: f32) -> anyhow::Result<()> {
        if self.refresh_interval_secs <= 0.0 {
            return Ok(());
        }
        let last_refresh = self.last_refresh_time_secs.unwrap_or(time_secs);
        if time_secs - last_refresh >= self.refresh_interval_secs {
            self.load_url_now()?;
            self.last_refresh_time_secs = Some(time_secs);
        }
        Ok(())
    }
}

impl Default for WebViewportRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectRenderer for WebViewportRenderer {
    fn init(&mut self, metadata: &EffectMetadata) -> anyhow::Result<()> {
        self.init_with_canvas_size(metadata, 640, 480)
    }

    fn init_with_canvas_size(
        &mut self,
        _metadata: &EffectMetadata,
        _canvas_width: u32,
        _canvas_height: u32,
    ) -> anyhow::Result<()> {
        self.destroy();
        let mut session = ServoSessionHandle::new_shared(self.session_config())?;
        let url = normalize_web_url_input(&self.url);
        if url.is_empty() {
            bail!("web viewport URL is empty");
        }
        if let Err(error) = session.load_url(&url) {
            note_servo_session_error("web viewport initial URL load failed", &error);
            self.load_failed = true;
            self.loaded_url = None;
            let _ = session.load_url(BLANK_URL);
        } else {
            self.url.clone_from(&url);
            self.loaded_url = Some(url);
            self.load_failed = false;
        }
        session.request_render(Vec::new())?;
        self.last_refresh_time_secs = Some(0.0);
        self.session = Some(session);
        Ok(())
    }

    fn render_into(&mut self, input: &FrameInput<'_>, canvas: &mut Canvas) -> anyhow::Result<()> {
        prepare_target_canvas(canvas, input.canvas_width, input.canvas_height);
        canvas.clear();

        if let Err(error) = self.maybe_load_pending_url() {
            note_servo_session_error("web viewport URL load failed", &error);
            return Ok(());
        }
        if let Err(error) = self.maybe_reload(input.time_secs) {
            note_servo_session_error("web viewport refresh failed", &error);
            return Ok(());
        }

        let mut latest_source = None::<Canvas>;
        if let Some(session) = self.session.as_mut() {
            match session.poll_frame() {
                Ok(Some(frame)) => {
                    self.preview_canvas = Some(frame.clone());
                    self.load_failed = false;
                    latest_source = Some(frame);
                }
                Ok(None) => {}
                Err(error) => {
                    note_servo_session_error("web viewport frame polling failed", &error);
                    return Err(error);
                }
            }
        }

        if latest_source.is_none() {
            latest_source = (!self.load_failed)
                .then(|| {
                    self.session
                        .as_ref()
                        .and_then(ServoSessionHandle::last_canvas)
                        .cloned()
                })
                .flatten();
        }

        if let Some(source) = latest_source.as_ref() {
            sample_viewport(
                canvas,
                source,
                self.viewport,
                self.fit_mode,
                self.brightness,
            );
        }

        let scripts = self.pending_scripts_for_render();
        let scroll_in_scripts = (!scripts.is_empty()).then_some((self.scroll_x, self.scroll_y));

        if let Some(session) = self.session.as_mut() {
            // `request_render` short-circuits (returns Ok without
            // queuing) when a prior render is still pending. Check
            // explicitly so we don't prematurely advance
            // `last_applied_scroll` for scripts that were dropped.
            let can_submit = !session.has_pending_render();
            if let Err(error) = session.request_render(scripts) {
                note_servo_session_error("web viewport render request failed", &error);
                return Err(error);
            }
            if can_submit && let Some(scroll) = scroll_in_scripts {
                self.last_applied_scroll = Some(scroll);
            }
        }

        Ok(())
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        match name {
            "url" => {
                if let ControlValue::Text(url) | ControlValue::Enum(url) = value {
                    let normalized = normalize_web_url_input(url);
                    if normalized != self.url {
                        self.url = normalized;
                        self.loaded_url = None;
                        self.load_failed = false;
                        self.preview_canvas = None;
                        self.url_dirty_at = Some(Instant::now());
                    }
                }
            }
            "viewport" => {
                if let ControlValue::Rect(rect) = value {
                    self.viewport = rect.clamp();
                }
            }
            "fit_mode" => {
                if let ControlValue::Enum(mode) | ControlValue::Text(mode) = value {
                    self.fit_mode = parse_fit_mode(mode);
                }
            }
            "brightness" => {
                if let Some(brightness) = value.as_f32() {
                    self.brightness = brightness.clamp(0.0, 2.0);
                }
            }
            "refresh_interval" => {
                if let Some(interval) = value.as_f32() {
                    self.refresh_interval_secs = interval.max(0.0);
                }
            }
            "render_width" => {
                if let Some(width) = value.as_f32() {
                    self.render_width = width.round().clamp(640.0, 1920.0) as u32;
                    if let Some(session) = self.session.as_mut() {
                        session.resize(self.render_width, self.render_height);
                    }
                }
            }
            "render_height" => {
                if let Some(height) = value.as_f32() {
                    self.render_height = height.round().clamp(360.0, 1080.0) as u32;
                    if let Some(session) = self.session.as_mut() {
                        session.resize(self.render_width, self.render_height);
                    }
                }
            }
            "scroll_x" => {
                if let Some(v) = value.as_f32() {
                    self.scroll_x = v.round().clamp(0.0, SCROLL_AXIS_MAX) as i32;
                }
            }
            "scroll_y" => {
                if let Some(v) = value.as_f32() {
                    self.scroll_y = v.round().clamp(0.0, SCROLL_AXIS_MAX) as i32;
                }
            }
            _ => {}
        }
    }

    fn preview_canvas(&self) -> Option<Canvas> {
        self.preview_canvas.clone()
    }

    fn destroy(&mut self) {
        if let Some(session) = self.session.take()
            && let Err(error) = session.close()
        {
            note_servo_session_error("web viewport session close failed", &error);
        }
        self.preview_canvas = None;
        self.loaded_url = None;
        self.url_dirty_at = None;
        self.load_failed = false;
        // Scroll is bound to the session, so a fresh session must
        // re-dispatch the current scroll on its first paint.
        self.last_applied_scroll = None;
    }
}

fn parse_fit_mode(value: &str) -> FitMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "contain" => FitMode::Contain,
        "stretch" => FitMode::Stretch,
        _ => FitMode::Cover,
    }
}

fn normalize_web_url_input(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() || has_url_scheme(trimmed) {
        return trimmed.to_owned();
    }

    if looks_like_host_input(trimmed) {
        format!("{}://{trimmed}", default_scheme_for_host_input(trimmed))
    } else {
        trimmed.to_owned()
    }
}

fn has_url_scheme(value: &str) -> bool {
    value.contains("://")
        || value.starts_with("file:")
        || value.starts_with("about:")
        || value.starts_with("data:")
}

fn looks_like_host_input(value: &str) -> bool {
    if value.contains(char::is_whitespace) || value.starts_with('/') || value.starts_with('#') {
        return false;
    }

    let host = value.split('/').next().unwrap_or(value);
    let host = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    let host_without_port = host.rsplit_once(':').map_or(host, |(left, right)| {
        if right.chars().all(|ch| ch.is_ascii_digit()) {
            left
        } else {
            host
        }
    });

    host_without_port.eq_ignore_ascii_case("localhost")
        || host_without_port.contains('.')
        || host_without_port.parse::<IpAddr>().is_ok()
}

fn default_scheme_for_host_input(value: &str) -> &'static str {
    let host = value.split('/').next().unwrap_or(value);
    let host = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    let host_without_port = host.rsplit_once(':').map_or(host, |(left, right)| {
        if right.chars().all(|ch| ch.is_ascii_digit()) {
            left
        } else {
            host
        }
    });

    let host_lower = host_without_port.to_ascii_lowercase();
    #[allow(
        clippy::case_sensitive_file_extension_comparisons,
        reason = "matching an mDNS hostname suffix, not a filesystem extension; the lowercase conversion above already normalizes case"
    )]
    let is_mdns_local = host_lower.ends_with(".local");
    if host_lower == "localhost"
        || is_mdns_local
        || host_without_port.parse::<IpAddr>().is_ok()
    {
        "http"
    } else {
        "https"
    }
}

fn controls() -> Vec<ControlDefinition> {
    vec![
        text_control(
            "url",
            "URL",
            "https://example.com",
            "Source",
            "HTTP, HTTPS, or file URL to render through Servo.",
        ),
        rect_control(
            "viewport",
            "Viewport",
            ViewportRect::full(),
            "Source",
            "Normalized crop region sampled from the rendered page.",
            PreviewSource::WebViewport,
            None,
        ),
        dropdown_control(
            "fit_mode",
            "Fit",
            "Cover",
            &["Contain", "Cover", "Stretch"],
            "Output",
            "How the selected viewport maps into the effect canvas.",
        ),
        slider_control(
            "brightness",
            "Brightness",
            1.0,
            0.0,
            2.0,
            0.01,
            "Output",
            "Brightness multiplier applied after sampling the viewport.",
        ),
        slider_control(
            "refresh_interval",
            "Refresh",
            0.0,
            0.0,
            300.0,
            1.0,
            "Source",
            "Seconds between forced reloads. Zero disables automatic refresh.",
        ),
        slider_control(
            "render_width",
            "Render Width",
            1280.0,
            640.0,
            1920.0,
            160.0,
            "Source",
            "Servo render width before viewport sampling.",
        ),
        slider_control(
            "render_height",
            "Render Height",
            720.0,
            360.0,
            1080.0,
            90.0,
            "Source",
            "Servo render height before viewport sampling.",
        ),
        slider_control(
            "scroll_x",
            "Scroll X",
            0.0,
            0.0,
            SCROLL_AXIS_MAX,
            1.0,
            "Source",
            "Horizontal scroll offset in page pixels before sampling.",
        ),
        slider_control(
            "scroll_y",
            "Scroll Y",
            0.0,
            0.0,
            SCROLL_AXIS_MAX,
            1.0,
            "Source",
            "Vertical scroll offset in page pixels before sampling.",
        ),
    ]
}

pub(super) fn metadata() -> EffectMetadata {
    EffectMetadata {
        id: builtin_effect_id("web_viewport"),
        name: "Web Viewport".into(),
        author: "Hypercolor".into(),
        version: "0.1.0".into(),
        description:
            "Loads a webpage in Servo and samples a draggable viewport from the rendered page."
                .into(),
        category: EffectCategory::Source,
        tags: vec![
            "web".into(),
            "browser".into(),
            "servo".into(),
            "viewport".into(),
            "source".into(),
        ],
        controls: controls(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from("builtin/web_viewport"),
        },
        license: Some("Apache-2.0".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        WebViewportRenderer, default_scheme_for_host_input, looks_like_host_input,
        normalize_web_url_input,
    };
    use crate::effect::traits::EffectRenderer;
    use hypercolor_types::effect::ControlValue;

    #[test]
    fn pending_scripts_are_empty_at_origin_with_no_dispatch_history() {
        let renderer = WebViewportRenderer::new();
        // Fresh renderer: scroll is (0, 0) and nothing has been applied
        // yet. Asking Servo to scrollTo(0, 0) every paint is wasted
        // work — the helper should skip the injection entirely.
        assert!(renderer.pending_scripts_for_render().is_empty());
    }

    #[test]
    fn pending_scripts_inject_absolute_scroll_when_nonzero() {
        let mut renderer = WebViewportRenderer::new();
        renderer.set_control("scroll_y", &ControlValue::Float(240.0));
        let scripts = renderer.pending_scripts_for_render();
        assert_eq!(scripts.len(), 1);
        // Absolute `scrollTo` with behaviour: 'instant' matches the
        // spec's promise that slider commits produce non-animated
        // scroll on the next Servo paint.
        assert!(
            scripts[0].contains("window.scrollTo(")
                && scripts[0].contains("left: 0")
                && scripts[0].contains("top: 240")
                && scripts[0].contains("behavior: 'instant'"),
            "script did not match expected shape: {:?}",
            scripts[0]
        );
    }

    #[test]
    fn pending_scripts_skip_when_scroll_unchanged_after_dispatch() {
        let mut renderer = WebViewportRenderer::new();
        renderer.set_control("scroll_y", &ControlValue::Float(500.0));
        // Simulate a previous successful dispatch (matches what
        // `render_into` records once the scripts reach Servo).
        renderer.last_applied_scroll = Some((0, 500));
        // Same scroll, no navigation in between — re-injecting would
        // just make Servo re-parse a no-op. Helper should return empty.
        assert!(renderer.pending_scripts_for_render().is_empty());
    }

    #[test]
    fn pending_scripts_re_inject_after_navigation_reset() {
        let mut renderer = WebViewportRenderer::new();
        renderer.set_control("scroll_y", &ControlValue::Float(500.0));
        renderer.last_applied_scroll = Some((0, 500));
        // Navigation (URL load / destroy) resets the dispatch memo;
        // `last_applied_scroll` returning to None should force a
        // re-inject on the next paint so the new page lands at the
        // user's intended scroll rather than the page's top.
        renderer.last_applied_scroll = None;
        assert_eq!(renderer.pending_scripts_for_render().len(), 1);
    }

    #[test]
    fn scroll_control_values_clamp_to_axis_max() {
        let mut renderer = WebViewportRenderer::new();
        renderer.set_control("scroll_x", &ControlValue::Float(-100.0));
        renderer.set_control("scroll_y", &ControlValue::Float(99_999.0));
        assert_eq!(renderer.scroll_x, 0);
        assert_eq!(renderer.scroll_y, 32_768);
    }

    #[test]
    fn normalize_web_url_input_trims_and_preserves_schemed_urls() {
        assert_eq!(
            normalize_web_url_input("  https://example.com/path  "),
            "https://example.com/path"
        );
        assert_eq!(
            normalize_web_url_input("file:///tmp/demo.html"),
            "file:///tmp/demo.html"
        );
    }

    #[test]
    fn normalize_web_url_input_prepends_https_for_bare_hosts() {
        assert_eq!(
            normalize_web_url_input("example.com/widgets"),
            "https://example.com/widgets"
        );
        assert_eq!(
            normalize_web_url_input("localhost:9430"),
            "http://localhost:9430"
        );
        assert_eq!(
            normalize_web_url_input("192.168.1.8:8080"),
            "http://192.168.1.8:8080"
        );
    }

    #[test]
    fn host_detection_skips_relative_and_fragment_inputs() {
        assert!(looks_like_host_input("127.0.0.1:8080"));
        assert!(!looks_like_host_input("/dashboard"));
        assert!(!looks_like_host_input("#anchor"));
        assert!(!looks_like_host_input("example path"));
    }

    #[test]
    fn local_hosts_default_to_http() {
        assert_eq!(default_scheme_for_host_input("localhost:3000"), "http");
        assert_eq!(default_scheme_for_host_input("127.0.0.1"), "http");
        assert_eq!(default_scheme_for_host_input("printer.local"), "http");
        assert_eq!(default_scheme_for_host_input("example.com"), "https");
    }
}
