use std::path::PathBuf;
use std::time::{Duration, Instant};

use hypercolor_types::canvas::Canvas;
use hypercolor_types::effect::{
    ControlDefinition, ControlValue, EffectCategory, EffectMetadata, EffectSource, PreviewSource,
};
use hypercolor_types::viewport::{FitMode, ViewportRect};

use super::common::{
    builtin_effect_id, dropdown_control, rect_control, slider_control, text_control,
};
use crate::effect::servo::{SessionConfig, ServoSessionHandle, note_servo_session_error};
use crate::effect::traits::{EffectRenderer, FrameInput, prepare_target_canvas};
use crate::spatial::sample_viewport;

const URL_LOAD_DEBOUNCE: Duration = Duration::from_millis(250);

pub struct WebViewportRenderer {
    session: Option<ServoSessionHandle>,
    url: String,
    viewport: ViewportRect,
    fit_mode: FitMode,
    brightness: f32,
    refresh_interval_secs: f32,
    render_width: u32,
    render_height: u32,
    last_refresh_time_secs: Option<f32>,
    url_dirty_at: Option<Instant>,
    loaded_url: Option<String>,
    preview_canvas: Option<Canvas>,
}

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
            last_refresh_time_secs: None,
            url_dirty_at: None,
            loaded_url: None,
            preview_canvas: None,
        }
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
        session.load_url(&self.url)?;
        self.loaded_url = Some(self.url.clone());
        self.url_dirty_at = None;
        Ok(())
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
        if let Err(error) = session.load_url(&self.url) {
            note_servo_session_error("web viewport initial URL load failed", &error);
            return Err(error);
        }
        session.request_render(Vec::new())?;
        self.loaded_url = Some(self.url.clone());
        self.last_refresh_time_secs = Some(0.0);
        self.session = Some(session);
        Ok(())
    }

    fn render_into(&mut self, input: &FrameInput<'_>, canvas: &mut Canvas) -> anyhow::Result<()> {
        prepare_target_canvas(canvas, input.canvas_width, input.canvas_height);
        canvas.clear();

        if let Err(error) = self.maybe_load_pending_url() {
            note_servo_session_error("web viewport URL load failed", &error);
            return Err(error);
        }
        if let Err(error) = self.maybe_reload(input.time_secs) {
            note_servo_session_error("web viewport refresh failed", &error);
            return Err(error);
        }

        let mut latest_source = None::<Canvas>;
        if let Some(session) = self.session.as_mut() {
            match session.poll_frame() {
                Ok(Some(frame)) => {
                    self.preview_canvas = Some(frame.clone());
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
            latest_source = self
                .session
                .as_ref()
                .and_then(ServoSessionHandle::last_canvas)
                .cloned();
        }

        if let Some(source) = latest_source.as_ref() {
            sample_viewport(canvas, source, self.viewport, self.fit_mode, self.brightness);
        }

        if let Some(session) = self.session.as_mut()
            && let Err(error) = session.request_render(Vec::new())
        {
            note_servo_session_error("web viewport render request failed", &error);
            return Err(error);
        }

        Ok(())
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        match name {
            "url" => {
                if let ControlValue::Text(url) | ControlValue::Enum(url) = value
                    && *url != self.url
                {
                    self.url = url.clone();
                    self.url_dirty_at = Some(Instant::now());
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
    }
}

fn parse_fit_mode(value: &str) -> FitMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "contain" => FitMode::Contain,
        "stretch" => FitMode::Stretch,
        _ => FitMode::Cover,
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
    ]
}

pub(super) fn metadata() -> EffectMetadata {
    EffectMetadata {
        id: builtin_effect_id("web_viewport"),
        name: "Web Viewport".into(),
        author: "Hypercolor".into(),
        version: "0.1.0".into(),
        description: "Loads a webpage in Servo and samples a draggable viewport from the rendered page.".into(),
        category: EffectCategory::Utility,
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
