//! Servo-backed HTML effect renderer (feature-gated).
//!
//! This is an integration scaffold that currently validates Servo context
//! initialization and Lightscript injection sequencing. Full WebView paint/
//! readback wiring is the next step.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Result, bail};
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::effect::{ControlValue, EffectMetadata, EffectSource};
use tracing::{debug, warn};

use super::bootstrap_software_rendering_context;
use super::lightscript::LightscriptRuntime;
use super::traits::{EffectRenderer, FrameInput};

/// Feature-gated renderer for HTML effects.
pub struct ServoRenderer {
    html_source: Option<PathBuf>,
    controls: HashMap<String, ControlValue>,
    runtime: LightscriptRuntime,
    initialized: bool,
    pending_scripts: Vec<String>,
    warned_placeholder_frame: bool,
}

impl ServoRenderer {
    /// Create a new Servo renderer instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            html_source: None,
            controls: HashMap::new(),
            runtime: LightscriptRuntime::new(320, 200),
            initialized: false,
            pending_scripts: Vec::new(),
            warned_placeholder_frame: false,
        }
    }

    fn enqueue_bootstrap_scripts(&mut self) {
        self.pending_scripts.push(self.runtime.bootstrap_script());
    }

    fn enqueue_frame_scripts(&mut self, input: &FrameInput) {
        let frame_scripts = self.runtime.frame_scripts(&input.audio, &self.controls);
        self.pending_scripts.push(frame_scripts.audio_update);
        self.pending_scripts.extend(frame_scripts.control_updates);
    }

    fn placeholder_canvas(&self, input: &FrameInput) -> Canvas {
        let mut canvas = Canvas::new(input.canvas_width, input.canvas_height);

        #[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
        let frame_mod = (input.frame_number % 255) as u8;
        #[expect(clippy::cast_possible_truncation, clippy::as_conversions)]
        let audio_mod = (input.audio.rms_level.clamp(0.0, 1.0) * 255.0) as u8;

        let color = Rgba::new(frame_mod, audio_mod, frame_mod.saturating_add(32), 255);
        canvas.fill(color);
        canvas
    }
}

impl EffectRenderer for ServoRenderer {
    fn init(&mut self, metadata: &EffectMetadata) -> Result<()> {
        let EffectSource::Html { path } = &metadata.source else {
            bail!(
                "ServoRenderer requires EffectSource::Html, got source {:?} for effect '{}'",
                metadata.source,
                metadata.name
            );
        };

        self.html_source = Some(path.clone());
        self.controls.clear();
        self.pending_scripts.clear();

        // Validate that Servo's software context can be created for this host.
        // We don't store it yet because `SoftwareRenderingContext` is not `Send`.
        let _ctx = bootstrap_software_rendering_context(320, 200)?;
        self.initialized = true;
        self.enqueue_bootstrap_scripts();

        debug!(
            effect = %metadata.name,
            source = %path.display(),
            "Initialized ServoRenderer scaffold"
        );

        Ok(())
    }

    fn tick(&mut self, input: &FrameInput) -> Result<Canvas> {
        if !self.initialized {
            bail!("ServoRenderer tick called before init");
        }

        self.enqueue_frame_scripts(input);

        if !self.warned_placeholder_frame {
            warn!(
                "ServoRenderer is currently returning placeholder frames; WebView paint/readback wiring is pending"
            );
            self.warned_placeholder_frame = true;
        }

        Ok(self.placeholder_canvas(input))
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        self.controls.insert(name.to_owned(), value.clone());
    }

    fn destroy(&mut self) {
        self.initialized = false;
        self.pending_scripts.clear();
        self.controls.clear();
        self.html_source = None;
        self.warned_placeholder_frame = false;
    }
}

impl Default for ServoRenderer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use hypercolor_types::audio::AudioData;
    use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
    use uuid::Uuid;

    use super::*;

    fn html_metadata() -> EffectMetadata {
        EffectMetadata {
            id: EffectId::new(Uuid::now_v7()),
            name: "html-test".to_owned(),
            author: "test".to_owned(),
            version: "0.1.0".to_owned(),
            description: "test html effect".to_owned(),
            category: EffectCategory::Ambient,
            tags: vec!["html".to_owned()],
            source: EffectSource::Html {
                path: PathBuf::from("community/test.html"),
            },
            license: None,
        }
    }

    #[test]
    fn init_and_tick_produce_frame() {
        let mut renderer = ServoRenderer::new();
        renderer.init(&html_metadata()).expect("init should work");

        let frame = renderer
            .tick(&FrameInput {
                time_secs: 0.0,
                delta_secs: 0.016,
                frame_number: 0,
                audio: AudioData::silence(),
                canvas_width: 320,
                canvas_height: 200,
            })
            .expect("tick should work");

        assert_eq!(frame.width(), 320);
        assert_eq!(frame.height(), 200);
    }
}
