use tracing::{debug, warn};

use hypercolor_core::input::{InteractionData, ScreenData};
use hypercolor_core::types::audio::AudioData;
use hypercolor_core::types::canvas::{Canvas, PublishedSurface, Rgba};
use hypercolor_types::sensor::SystemSnapshot;

use super::RenderThreadState;
use super::pipeline_runtime::{CachedStaticSurface, StaticSurfaceKey};

fn static_hold_canvas(width: u32, height: u32, color: [u8; 3]) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    if color != [0, 0, 0] {
        canvas.fill(Rgba::new(color[0], color[1], color[2], 255));
    }
    canvas
}

pub(crate) fn static_surface(
    cache: &mut Option<CachedStaticSurface>,
    width: u32,
    height: u32,
    color: [u8; 3],
) -> PublishedSurface {
    let key = StaticSurfaceKey {
        width,
        height,
        color,
    };

    if let Some(cached) = cache.as_ref()
        && cached.key == key
    {
        return cached.surface.clone();
    }

    let surface =
        PublishedSurface::from_owned_canvas(static_hold_canvas(width, height, color), 0, 0);
    *cache = Some(CachedStaticSurface {
        key,
        surface: surface.clone(),
    });
    surface
}

pub(crate) async fn render_effect_into(
    state: &RenderThreadState,
    expected_generation: u64,
    delta_secs: f32,
    audio: &AudioData,
    interaction: &InteractionData,
    screen: Option<&ScreenData>,
    sensors: &SystemSnapshot,
    target: &mut Canvas,
) -> Option<Canvas> {
    let mut engine = state.effect_engine.lock().await;
    let actual_generation = engine.scene_generation();

    if actual_generation != expected_generation {
        debug!(
            expected_generation,
            actual_generation, "deferred effect render until next frame after scene change"
        );
        if target.width() != state.canvas_dims.width()
            || target.height() != state.canvas_dims.height()
        {
            *target = Canvas::new(state.canvas_dims.width(), state.canvas_dims.height());
        } else {
            target.clear();
        }
        return None;
    }

    match engine.tick_with_inputs_and_sensors_into(
        delta_secs,
        audio,
        interaction,
        screen,
        sensors,
        target,
    ) {
        Ok(()) => {
            if state.event_bus.web_viewport_canvas_receiver_count() > 0 {
                engine.preview_canvas()
            } else {
                None
            }
        }
        Err(error) => {
            warn!(%error, "effect render failed, producing black canvas");
            if target.width() != state.canvas_dims.width()
                || target.height() != state.canvas_dims.height()
            {
                *target = Canvas::new(state.canvas_dims.width(), state.canvas_dims.height());
            } else {
                target.clear();
            }
            None
        }
    }
}
