use std::time::SystemTime;

use anyhow::Result;

use hypercolor_core::overlay::{OverlayBuffer, OverlayInput, OverlayRenderer, OverlaySize};
use hypercolor_types::sensor::SystemSnapshot;

struct DummyRenderer;

impl OverlayRenderer for DummyRenderer {
    fn init(&mut self, _target_size: OverlaySize) -> Result<()> {
        Ok(())
    }

    fn resize(&mut self, _target_size: OverlaySize) -> Result<()> {
        Ok(())
    }

    fn render_into(
        &mut self,
        _input: &OverlayInput<'_>,
        target: &mut OverlayBuffer,
    ) -> std::result::Result<(), hypercolor_core::overlay::OverlayError> {
        target.clear();
        Ok(())
    }
}

#[test]
fn overlay_buffer_resizes_and_clears() {
    let mut buffer = OverlayBuffer::new(OverlaySize::new(4, 2));
    assert_eq!(buffer.pixels.len(), 32);

    buffer.pixels.fill(255);
    buffer.clear();
    assert!(buffer.pixels.iter().all(|value| *value == 0));

    buffer.resize(OverlaySize::new(2, 2));
    assert_eq!(buffer.width, 2);
    assert_eq!(buffer.height, 2);
    assert_eq!(buffer.pixels.len(), 16);
}

#[test]
fn overlay_renderer_trait_is_object_safe() {
    let mut renderer: Box<dyn OverlayRenderer> = Box::new(DummyRenderer);
    renderer
        .init(OverlaySize::new(2, 2))
        .expect("init should succeed");

    let mut buffer = OverlayBuffer::new(OverlaySize::new(2, 2));
    let sensors = SystemSnapshot::empty();
    let input = OverlayInput {
        now: SystemTime::now(),
        display_width: 2,
        display_height: 2,
        circular: false,
        sensors: &sensors,
        elapsed_secs: 0.0,
        frame_number: 1,
    };
    renderer
        .render_into(&input, &mut buffer)
        .expect("render should succeed");
}
