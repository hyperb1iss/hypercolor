use std::alloc::System;

use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

fn area_layout() -> SpatialLayout {
    SpatialLayout {
        id: "area-reuse".into(),
        name: "Area Reuse".into(),
        description: None,
        canvas_width: 8,
        canvas_height: 6,
        zones: vec![Output {
            id: "area".into(),
            name: "area".into(),
            device_id: "test:area".into(),
            zone_name: None,
            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(0.0, 0.0),
            rotation: 0.0,
            scale: 1.0,
            display_order: 0,
            orientation: None,
            topology: LedTopology::Point,
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: Some(SamplingMode::AreaAverage {
                radius_x: 2.0,
                radius_y: 1.0,
            }),
            edge_behavior: Some(EdgeBehavior::Clamp),
            shape: None,
            shape_preset: None,
            attachment: None,
            brightness: None,
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn canvas() -> Canvas {
    let mut canvas = Canvas::new(8, 6);
    for y in 0..6_u32 {
        for x in 0..8_u32 {
            let byte = (x + y * 8).to_le_bytes()[0];
            canvas.set_pixel(x, y, Rgba::new(byte, byte.wrapping_mul(3), 255 - byte, 255));
        }
    }
    canvas
}

#[test]
fn canonical_sampling_reuses_the_workspace_without_allocating() {
    let engine = SpatialEngine::try_new(area_layout()).expect("canonical workspace prepares");
    let canvas = canvas();
    let mut zones = Vec::<ZoneColors>::new();
    engine
        .try_sample_into(&canvas, &mut zones)
        .expect("warm sample succeeds");

    let mut region = Region::new(GLOBAL);
    region.reset();
    engine
        .try_sample_into(&canvas, &mut zones)
        .expect("steady sample succeeds");
    let stats = region.change();

    assert_eq!(stats.allocations, 0);
    assert_eq!(stats.reallocations, 0);
    assert_eq!(stats.deallocations, 0);
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].colors.len(), 1);
}
