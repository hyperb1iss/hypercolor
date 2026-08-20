use std::sync::LazyLock;

use anyhow::Result;

use hypercolor_core::effect::{EffectRenderer, FrameDataSources, FrameInput};
use hypercolor_core::input::{InteractionData, ScreenData};
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::effect::EffectMetadata;
use hypercolor_types::sensor::SystemSnapshot;

static EMPTY_SENSORS: LazyLock<SystemSnapshot> = LazyLock::new(SystemSnapshot::empty);

pub struct TestFrameState {
    elapsed_secs: f64,
    frame_number: u64,
    canvas_width: u32,
    canvas_height: u32,
}

impl TestFrameState {
    pub const fn new(canvas_width: u32, canvas_height: u32) -> Self {
        Self {
            elapsed_secs: 0.0,
            frame_number: 0,
            canvas_width,
            canvas_height,
        }
    }

    pub fn initialize(
        &self,
        renderer: &mut dyn EffectRenderer,
        metadata: &EffectMetadata,
    ) -> Result<()> {
        renderer.init_with_canvas_size(metadata, self.canvas_width, self.canvas_height)
    }

    pub fn render(
        &mut self,
        renderer: &mut dyn EffectRenderer,
        delta_secs: f32,
        audio: &AudioData,
    ) -> Result<Canvas> {
        let mut canvas = Canvas::new(self.canvas_width, self.canvas_height);
        self.render_with_inputs_into(
            renderer,
            delta_secs,
            audio,
            &InteractionData::default(),
            None,
            &EMPTY_SENSORS,
            &mut canvas,
        )?;
        Ok(canvas)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "test rendering exposes the same frame inputs as EffectRenderer"
    )]
    pub fn render_with_inputs_into(
        &mut self,
        renderer: &mut dyn EffectRenderer,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        target: &mut Canvas,
    ) -> Result<()> {
        self.elapsed_secs += f64::from(delta_secs);
        let input = FrameInput {
            time_secs: self.elapsed_secs,
            delta_secs,
            frame_number: self.frame_number,
            audio,
            interaction,
            screen,
            sensors,
            sources: FrameDataSources::default(),
            canvas_width: self.canvas_width,
            canvas_height: self.canvas_height,
        };
        renderer.render_into(&input, target)?;
        self.frame_number = self.frame_number.wrapping_add(1);
        Ok(())
    }
}
