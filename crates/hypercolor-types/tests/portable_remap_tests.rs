use std::collections::BTreeMap;

use hypercolor_types::portable_setup::PortableSetupV1;
use uuid::Uuid;

fn fixture() -> PortableSetupV1 {
    serde_json::from_str(include_str!("fixtures/portable_setup_v1.json"))
        .expect("valid portable fixture")
}

#[test]
fn translates_every_device_reference_and_preserves_other_authored_identities() {
    let original = fixture();
    let physical = *original.devices.keys().next().expect("physical device");
    let logical = *original
        .logical_devices
        .keys()
        .next()
        .expect("logical device");
    // An effect identity is unrelated to the physical/logical namespace.
    let new_physical = *original.effects.keys().next().expect("effect");
    let new_logical = Uuid::from_u128(22);
    let remapped = original
        .remap_devices_and_outputs(&BTreeMap::from([
            (physical, new_physical),
            (logical, new_logical),
        ]))
        .expect("distinct device destinations");

    assert_eq!(remapped.devices[&new_physical], original.devices[&physical]);
    assert_eq!(remapped.logical_devices[&new_logical].device, new_physical);
    assert_eq!(
        remapped.attachments[&new_physical],
        original.attachments[&physical]
    );
    assert_eq!(
        remapped.display_faces[&new_physical],
        original.display_faces[&physical]
    );
    assert_eq!(
        remapped.device_output[&new_physical],
        original.device_output[&physical]
    );
    assert_eq!(remapped.layouts["case"].zones[0].device, new_logical);
    for (id, scene) in &remapped.scenes {
        let previous = &original.scenes[id];
        assert_eq!(scene.zones[0].layout.zones[0].device, new_logical);
        assert_eq!(
            scene.zones[0]
                .display_target
                .as_ref()
                .expect("display")
                .device,
            new_physical
        );
        assert_eq!(scene.zones[0].id, previous.zones[0].id);
        assert_eq!(scene.zones[0].layers, previous.zones[0].layers);
        assert_eq!(scene.layout_id, previous.layout_id);
    }
    assert_eq!(remapped.setup_id, original.setup_id);
    assert_eq!(remapped.effects, original.effects);
    assert_eq!(remapped.assets, original.assets);
    assert_eq!(remapped.presets, original.presets);
    assert_eq!(remapped.playlists, original.playlists);
    assert_eq!(remapped.attachment_templates, original.attachment_templates);
    assert_eq!(remapped.selection, original.selection);
    assert_eq!(remapped.output, original.output);
    assert_eq!(remapped.layouts["case"].id, "case");
    assert_eq!(remapped.layouts["case"].zones[0].id, "front");
    assert_eq!(
        remapped
            .remap_devices_and_outputs(&BTreeMap::from([
                (new_physical, physical),
                (new_logical, logical),
            ]))
            .expect("inverse mapping"),
        original,
    );
}

#[test]
fn swaps_device_identities_simultaneously_without_cascading() {
    let original = fixture();
    let physical = *original.devices.keys().next().expect("physical device");
    let logical = *original
        .logical_devices
        .keys()
        .next()
        .expect("logical device");
    let remapped = original
        .remap_devices_and_outputs(&BTreeMap::from([(physical, logical), (logical, physical)]))
        .expect("bijective swap");
    assert_eq!(remapped.devices[&logical], original.devices[&physical]);
    assert_eq!(remapped.logical_devices[&physical].device, logical);
    assert_eq!(remapped.layouts["case"].zones[0].device, physical);
}

#[test]
fn rejects_collisions_with_mapped_and_unchanged_device_identities() {
    let mut original = fixture();
    let physical = *original.devices.keys().next().expect("physical device");
    let logical = *original
        .logical_devices
        .keys()
        .next()
        .expect("logical device");
    let other = Uuid::from_u128(23);
    original
        .devices
        .insert(other, original.devices[&physical].clone());
    let shared = Uuid::from_u128(24);
    for mapping in [
        BTreeMap::from([(physical, shared), (other, shared)]),
        BTreeMap::from([(physical, other)]),
        BTreeMap::from([(physical, logical)]),
        BTreeMap::from([(logical, physical)]),
        BTreeMap::from([(physical, Uuid::nil())]),
    ] {
        assert!(original.remap_devices_and_outputs(&mapping).is_err());
    }
    original
        .validate()
        .expect("failed mapping leaves original graph valid");
}

#[test]
fn partial_mapping_preserves_unmatched_ids_and_ignores_unrelated_namespaces() {
    let original = fixture();
    let physical = *original.devices.keys().next().expect("physical device");
    let logical = *original
        .logical_devices
        .keys()
        .next()
        .expect("logical device");
    let scene = *original.scenes.keys().next().expect("scene");
    let remapped = original
        .remap_devices_and_outputs(&BTreeMap::from([
            (physical, Uuid::from_u128(25)),
            (scene, Uuid::nil()),
            (Uuid::from_u128(26), logical),
        ]))
        .expect("unrelated mappings are unused");
    assert!(remapped.logical_devices.contains_key(&logical));
    assert_eq!(remapped.layouts["case"].zones[0].device, logical);
    assert!(remapped.scenes.contains_key(&scene));
    assert_eq!(remapped.selection, original.selection);
    assert_eq!(
        original
            .remap_devices_and_outputs(&BTreeMap::new())
            .expect("identity mapping"),
        original
    );
}

#[test]
fn refuses_dangling_input_references_instead_of_repairing_them_by_alias() {
    let mut original = fixture();
    let physical = *original.devices.keys().next().expect("physical device");
    let missing = Uuid::from_u128(27);
    original
        .logical_devices
        .values_mut()
        .next()
        .expect("logical device")
        .device = missing;
    assert!(
        original
            .remap_devices_and_outputs(&BTreeMap::from([(missing, physical)]))
            .is_err()
    );
}
