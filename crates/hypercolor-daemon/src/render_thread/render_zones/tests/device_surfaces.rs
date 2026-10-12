use super::*;

use hypercolor_types::device::DeviceId;

fn reconcile_zones(runtime: &mut ZoneRuntime, zones: &[Zone], revision: u64) {
    runtime
        .reconcile(
            zones,
            Some(SceneId::DEFAULT),
            SceneDependencyKey::new(revision, 1),
            &EffectRegistry::default(),
            &HashMap::new(),
            None,
        )
        .expect("touch surface zones should reconcile");
}

fn zone_with_pad(x: f32, y: f32) -> Zone {
    let mut zone = sample_zone(64, 64);
    make_color_fill_zone(&mut zone);
    zone.layout.zones.push(point_zone_at("pad", x, y));
    zone
}

fn canvas_center(
    runtime: &mut ZoneRuntime,
    zone: &Zone,
    device: DeviceId,
) -> Option<NormalizedPosition> {
    runtime
        .device_surfaces_for(zone)
        .and_then(|surfaces| surfaces.canvas_position(device, 0.5, 0.5))
}

#[test]
fn zone_surfaces_resolve_through_the_routed_layout_bindings() {
    let mut runtime = ZoneRuntime::try_new(64, 64).expect("small runtime should construct");
    let pad = DeviceId::new();
    let zone = zone_with_pad(0.25, 0.75);
    reconcile_zones(&mut runtime, std::slice::from_ref(&zone), 1);

    assert!(
        runtime.device_surfaces_for(&zone).is_none(),
        "no bindings observed yet, so nothing is placed"
    );

    runtime.observe_device_bindings(1, [("pad", pad)].into_iter());
    assert_eq!(
        canvas_center(&mut runtime, &zone, pad),
        Some(NormalizedPosition::new(0.25, 0.75))
    );
}

#[test]
fn zone_without_the_device_reports_no_surfaces() {
    let mut runtime = ZoneRuntime::try_new(64, 64).expect("small runtime should construct");
    let pad = DeviceId::new();
    let mut other = sample_zone(64, 64);
    make_color_fill_zone(&mut other);
    other.layout.zones.push(point_zone("strip"));
    reconcile_zones(&mut runtime, std::slice::from_ref(&other), 1);

    runtime.observe_device_bindings(1, [("pad", pad)].into_iter());
    assert!(runtime.device_surfaces_for(&other).is_none());
}

#[test]
fn rebinding_and_relayout_move_the_surface() {
    let mut runtime = ZoneRuntime::try_new(64, 64).expect("small runtime should construct");
    let first = DeviceId::new();
    let second = DeviceId::new();
    let zone = zone_with_pad(0.25, 0.25);
    reconcile_zones(&mut runtime, std::slice::from_ref(&zone), 1);
    runtime.observe_device_bindings(1, [("pad", first)].into_iter());
    assert!(canvas_center(&mut runtime, &zone, first).is_some());

    // An unchanged generation keeps the bindings it already has.
    runtime.observe_device_bindings(1, [("pad", second)].into_iter());
    assert!(canvas_center(&mut runtime, &zone, second).is_none());

    // A new generation rebinds the layout output to another device.
    runtime.observe_device_bindings(2, [("pad", second)].into_iter());
    assert!(canvas_center(&mut runtime, &zone, first).is_none());
    assert_eq!(
        canvas_center(&mut runtime, &zone, second),
        Some(NormalizedPosition::new(0.25, 0.25))
    );

    // Moving the output in the zone's layout moves the touch with it.
    let mut moved = zone.clone();
    moved.layout.zones[0].position = NormalizedPosition::new(0.75, 0.5);
    reconcile_zones(&mut runtime, std::slice::from_ref(&moved), 2);
    assert_eq!(
        canvas_center(&mut runtime, &moved, second),
        Some(NormalizedPosition::new(0.75, 0.5))
    );
}
