use hypercolor_core::spatial::SpatialEngine;
use hypercolor_daemon::domain::spatial::SpatialService;
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

fn layout(id: &str, width: u32) -> SpatialLayout {
    SpatialLayout {
        id: id.to_owned(),
        name: id.to_owned(),
        description: None,
        canvas_width: width,
        canvas_height: 120,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

#[test]
fn cloned_services_return_owned_spatial_snapshots() {
    let service = SpatialService::new(
        SpatialEngine::try_new(layout("initial", 160)).expect("initial layout should prepare"),
    );
    let clone = service.clone();
    let snapshot = service.snapshot();
    assert_eq!(snapshot.layout().id, "initial");
    assert_eq!(clone.layout().id, "initial");
    assert_eq!(clone.reader().load().layout().id, "initial");
}
