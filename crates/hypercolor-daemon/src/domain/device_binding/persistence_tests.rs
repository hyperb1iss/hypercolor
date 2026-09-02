use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::scene::{SceneManager, default_primary_zone, make_scene};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::attachment::DeviceComponentProfile;
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFingerprint,
    DeviceId, DeviceInfo, DeviceOrigin, DeviceTopologyHint, SegmentInfo,
};
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::BlendMode;
use hypercolor_types::scene::{DisplayFaceTarget, ZoneRole};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use tokio::sync::RwLock;

use super::{
    BindingClass, CurrentBinding, DeviceBindingMigrationContext, DeviceBindingRemaps,
    MigrationPersistence, PersistedBindingEvidence, SegmentShape, build_binding_remaps,
    plan_layout_device_id_remaps,
};
use crate::attachment_profiles::ComponentProfileStore;
use crate::device_binding_journal::DeviceBindingMigrationJournal;
use crate::device_settings::{DeviceSettingsStore, StoredDeviceSettings};
use crate::display_preferences::{DisplayPreference, DisplayPreferencesStore};
use crate::domain::layout::LayoutContext;
use crate::domain::scene::SceneService;
use crate::domain::spatial::SpatialService;
use crate::layout_auto_exclusions::LayoutAutoExclusionKey;
use crate::logical_devices::{LogicalDevice, LogicalDeviceKind};
use crate::output_power::OutputPower;
use crate::scene_transactions::{LayoutTransactionRejection, SceneTransactionQueue};
use crate::zone_layout_preview::ZoneLayoutPreviewStore;

const LEGACY_LAYOUT_ID: &str = "razer:1532:0099:001-6-4-4";
const CANONICAL_LAYOUT_ID: &str = "razer:1532:0099:pci-root";
const LEGACY_FINGERPRINT: &str = "usb:razer:1532:0099:001-6-4-4";
const CANONICAL_FINGERPRINT: &str = "usb:razer:1532:0099:pci-root";

fn layout(id: &str, device_id: Option<&str>) -> SpatialLayout {
    let zones = device_id.into_iter().map(output).collect();
    SpatialLayout {
        id: id.to_owned(),
        name: id.to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn output(device_id: &str) -> Output {
    Output {
        id: "main".to_owned(),
        name: "Main".to_owned(),
        device_id: device_id.to_owned(),
        zone_name: Some("Main".to_owned()),
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(1.0, 1.0),
        rotation: 0.0,
        scale: 1.0,
        display_order: 0,
        orientation: None,
        topology: LedTopology::Strip {
            count: 16,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        attachment: None,
        brightness: None,
    }
}

fn current_binding(canonical_physical_id: DeviceId) -> CurrentBinding {
    let device_info = DeviceInfo {
        id: canonical_physical_id,
        name: "Razer Device".to_owned(),
        vendor: "Razer".to_owned(),
        family: DeviceFamily::new("razer", "Razer"),
        model: Some("razer_test_device".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("razer", "usb", ConnectionType::Usb),
        segments: vec![SegmentInfo {
            name: "Main".to_owned(),
            led_count: 16,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: 16,
            ..DeviceCapabilities::default()
        },
    };
    CurrentBinding {
        layout_device_id: CANONICAL_LAYOUT_ID.to_owned(),
        physical_device_id: canonical_physical_id,
        fingerprint: DeviceFingerprint::from_persisted(CANONICAL_FINGERPRINT),
        device_info,
        class: BindingClass::ClaimlessUsb {
            owner: "razer".to_owned(),
            vendor_id: 0x1532,
            product_id: 0x0099,
        },
        segment_shape: vec![SegmentShape {
            name: "main".to_owned(),
            led_count: 16,
        }],
        display_surface_shapes: HashSet::new(),
    }
}

#[tokio::test]
async fn transaction_preserves_layout_evidence_until_dependents_are_durable() {
    let temp = tempfile::tempdir().expect("device binding state directory");
    let layouts_path = temp.path().join("layouts.json");
    let exclusions_path = temp.path().join("layout-auto-exclusions.json");
    let scenes_path = temp.path().join("scenes.json");
    let runtime_path = temp.path().join("runtime-state.json");
    let logical_path = temp.path().join("logical-devices.json");
    let profiles_path = temp.path().join("attachment-profiles.json");
    let settings_path = temp.path().join("device-settings.json");
    let preferences_path = temp.path().join("display-preferences.json");
    let journal_path = temp.path().join("device-binding-migration.json");

    let legacy_physical_id =
        DeviceFingerprint::from_persisted(LEGACY_FINGERPRINT).stable_device_id();
    let canonical_physical_id =
        DeviceFingerprint::from_persisted(CANONICAL_FINGERPRINT).stable_device_id();
    let legacy_layout = layout("saved", Some(LEGACY_LAYOUT_ID));
    crate::layout_store::save(
        &layouts_path,
        &HashMap::from([("saved".to_owned(), legacy_layout.clone())]),
    )
    .expect("initial legacy layout");

    let mut manager = SceneManager::with_default_layout(legacy_layout.clone());
    let mut named = make_scene("Imported Linux scene");
    let mut named_zone = default_primary_zone(legacy_layout.clone());
    named_zone.display_target = Some(DisplayFaceTarget::new(legacy_physical_id));
    named_zone.role = ZoneRole::Display;
    named.zones.push(named_zone);
    manager.create(named).expect("named scene should be valid");

    let event_bus = Arc::new(HypercolorBus::new());
    let scenes = SceneService::new(
        manager,
        Arc::clone(&event_bus),
        crate::scene_store::load_for_test(&scenes_path).expect("scene store"),
        Arc::new(ZoneLayoutPreviewStore::default()),
    );
    let spatial = SpatialService::new(SpatialEngine::new(legacy_layout.clone()));
    let layout_context = LayoutContext::new_test_context(
        HashMap::from([("saved".to_owned(), legacy_layout)]),
        layouts_path.clone(),
        HashMap::from([(
            LayoutAutoExclusionKey::layout("saved"),
            HashSet::from([LEGACY_LAYOUT_ID.to_owned()]),
        )]),
        exclusions_path.clone(),
        spatial.clone(),
        scenes.clone(),
        SceneTransactionQueue::default(),
        runtime_path.clone(),
    );

    let logical_devices = Arc::new(RwLock::new(HashMap::from([(
        LEGACY_LAYOUT_ID.to_owned(),
        LogicalDevice {
            id: LEGACY_LAYOUT_ID.to_owned(),
            physical_device_id: legacy_physical_id,
            name: "Imported segment".to_owned(),
            led_start: 0,
            led_count: 16,
            enabled: true,
            kind: LogicalDeviceKind::Segment,
        },
    )])));
    let mut profiles = ComponentProfileStore::new(profiles_path.clone());
    profiles.update(
        &legacy_physical_id.to_string(),
        DeviceComponentProfile::default(),
    );
    let profiles = Arc::new(RwLock::new(profiles));

    let output_power = OutputPower::new(DeviceSettingsStore::new(settings_path.clone()));
    let settings = output_power.device_settings();
    settings
        .persist_device_settings(
            LEGACY_FINGERPRINT,
            StoredDeviceSettings {
                name: Some("Imported keyboard".to_owned()),
                disabled: false,
                brightness: 0.5,
            },
        )
        .await
        .expect("legacy device settings");
    settings
        .persist_driver_control_values(
            &legacy_physical_id.to_string(),
            hypercolor_types::controls::ControlValueMap::from([(
                "enabled".into(),
                ControlValue::Bool(true),
            )]),
        )
        .await
        .expect("legacy driver controls");

    let mut preferences =
        DisplayPreferencesStore::new(preferences_path.clone()).expect("display preferences");
    preferences
        .set(
            legacy_physical_id,
            DisplayPreference {
                effect_id: EffectId::new(uuid::Uuid::nil()),
                controls: HashMap::new(),
                blend_mode: BlendMode::Alpha,
                opacity: 1.0,
            },
        )
        .expect("legacy display preference");
    let preferences = Arc::new(RwLock::new(preferences));

    let context = DeviceBindingMigrationContext::new(
        layout_context.clone(),
        Arc::clone(&logical_devices),
        logical_path.clone(),
        Arc::clone(&profiles),
        settings.clone(),
        Arc::clone(&preferences),
        journal_path.clone(),
    );
    let renderer = layout_context.layout_publication_test_executor();
    let remaps = DeviceBindingRemaps {
        layout_device_ids: HashMap::from([(
            LEGACY_LAYOUT_ID.to_owned(),
            CANONICAL_LAYOUT_ID.to_owned(),
        )]),
        physical_device_ids: HashMap::from([(legacy_physical_id, canonical_physical_id)]),
        persisted_setting_keys: HashMap::from([
            (
                LEGACY_FINGERPRINT.to_owned(),
                CANONICAL_FINGERPRINT.to_owned(),
            ),
            (
                legacy_physical_id.to_string(),
                canonical_physical_id.to_string(),
            ),
        ]),
    };
    context
        .journal
        .persist_active(&remaps)
        .expect("persist migration journal");

    let prepared = context.prepare(&remaps).await.expect("prepare migration");
    let profile_writer = crate::persistence::AtomicFileWriter::new(&profiles_path)
        .expect("attachment profile writer");
    profile_writer.set_injected_replace_failures(1);
    let (persisted, persistence) = prepared.persist();
    assert!(persisted.is_none());
    assert!(
        persistence
            .iter()
            .any(|outcome| matches!(outcome, MigrationPersistence::Failed(_)))
    );
    profile_writer
        .flush(Duration::from_secs(5))
        .expect("failed participant retry should converge");
    let layouts = crate::layout_store::load(&layouts_path).expect("reload legacy layouts");
    assert_eq!(layouts["saved"].zones[0].device_id, LEGACY_LAYOUT_ID);
    assert_eq!(
        DeviceBindingMigrationJournal::new(journal_path.clone())
            .load()
            .expect("reload active migration journal"),
        Some(remaps.clone())
    );

    let prepared = context
        .prepare(&remaps)
        .await
        .expect("prepare partial migration");
    let exclusions_writer = crate::persistence::AtomicFileWriter::new(&exclusions_path)
        .expect("layout exclusions writer");
    exclusions_writer.set_injected_replace_failures(1);
    let (persisted, persistence) = prepared.persist();
    assert!(persisted.is_none());
    assert!(
        persistence
            .iter()
            .any(|outcome| matches!(outcome, MigrationPersistence::Failed(_)))
    );

    let layouts = crate::layout_store::load(&layouts_path).expect("reload mixed layouts");
    assert_eq!(layouts["saved"].zones[0].device_id, LEGACY_LAYOUT_ID);
    let stored_scenes = crate::scene_store::load(&scenes_path).expect("reload migrated scenes");
    let mut mixed_evidence = PersistedBindingEvidence::default();
    mixed_evidence.observe_layout(&layouts["saved"]);
    for scene in stored_scenes.list() {
        for zone in &scene.zones {
            mixed_evidence.observe_layout(&zone.layout);
        }
    }
    assert!(
        plan_layout_device_id_remaps(&mixed_evidence, &[current_binding(canonical_physical_id)])
            .is_empty(),
        "mixed canonical and legacy evidence cannot reconstruct the remap"
    );
    let replayed_remaps = DeviceBindingMigrationJournal::new(journal_path.clone())
        .load()
        .expect("reload restart migration journal")
        .expect("restart must retain active remaps");
    assert_eq!(replayed_remaps, remaps);

    let prepared = context
        .prepare(&replayed_remaps)
        .await
        .expect("prepare journal replay");
    let (persisted, persistence) = prepared.persist();
    assert!(
        persistence
            .iter()
            .all(|outcome| matches!(outcome, MigrationPersistence::Durable))
    );
    let persisted = persisted.expect("all participants should persist");
    let (convergence, rejected) = tokio::join!(
        context
            .layout
            .converge_persisted_device_binding(&persisted.layout),
        async {
            while renderer.pending_layout_publications() == 0 {
                tokio::task::yield_now().await;
            }
            renderer.reject_next_layout_publication(LayoutTransactionRejection::RendererStopped)
        }
    );
    assert!(rejected);
    assert!(convergence.is_err());
    drop(persisted);
    assert_eq!(
        context
            .journal
            .load()
            .expect("reload journal after renderer rejection"),
        Some(remaps.clone())
    );
    assert!(
        logical_devices.read().await.contains_key(LEGACY_LAYOUT_ID),
        "memory must remain unpublished after renderer rejection"
    );

    let prepared = context
        .prepare(&remaps)
        .await
        .expect("prepare renderer retry");
    let (persisted, persistence) = prepared.persist();
    assert!(
        persistence
            .iter()
            .all(|outcome| matches!(outcome, MigrationPersistence::Durable))
    );
    let persisted = persisted.expect("renderer retry should persist");
    let (convergence, rendered) = tokio::join!(
        context
            .layout
            .converge_persisted_device_binding(&persisted.layout),
        async {
            while renderer.pending_layout_publications() == 0 {
                tokio::task::yield_now().await;
            }
            renderer.execute_next_layout_publication().await
        }
    );
    convergence.expect("renderer convergence");
    let rendered = rendered
        .expect("renderer publication")
        .expect("renderer should apply the active layout");
    assert_eq!(rendered.zones[0].device_id, CANONICAL_LAYOUT_ID);
    assert_eq!(
        spatial.layout().zones[0].device_id,
        LEGACY_LAYOUT_ID,
        "renderer-only admission must not publish domain memory"
    );
    let mut publication = context
        .prepare_publication(persisted)
        .await
        .expect("prepare renderer retry publication");
    let mut events = event_bus.subscribe_all();
    assert_eq!(publication.publish(&context), 11);
    let mut layout_changes = Vec::new();
    while let Ok(timestamped) = events.try_recv() {
        if let hypercolor_types::event::HypercolorEvent::LayoutChanged { previous, current } =
            timestamped.event
        {
            layout_changes.push((previous, current));
        }
    }
    assert_eq!(
        layout_changes,
        vec![(None, "saved".to_owned())],
        "the migration rebinds one stored layout and names it once"
    );
    context.journal.clear().expect("clear migration journal");
    assert!(
        context
            .journal
            .load()
            .expect("reload cleared migration journal")
            .is_none()
    );

    let layouts = crate::layout_store::load(&layouts_path).expect("reload layouts");
    assert_eq!(layouts["saved"].zones[0].device_id, CANONICAL_LAYOUT_ID);
    let exclusions =
        crate::layout_auto_exclusions::load(&exclusions_path).expect("reload layout exclusions");
    assert!(exclusions[&LayoutAutoExclusionKey::layout("saved")].contains(CANONICAL_LAYOUT_ID));
    assert!(!exclusions[&LayoutAutoExclusionKey::layout("saved")].contains(LEGACY_LAYOUT_ID));

    let stored_scenes = crate::scene_store::load(&scenes_path).expect("reload scenes");
    let stored_zone = &stored_scenes
        .list()
        .next()
        .expect("stored named scene")
        .zones[0];
    assert_eq!(stored_zone.layout.zones[0].device_id, CANONICAL_LAYOUT_ID);
    assert_eq!(
        stored_zone
            .display_target
            .as_ref()
            .expect("stored display target")
            .device_id,
        canonical_physical_id
    );
    let runtime = crate::runtime_state::load(&runtime_path)
        .expect("reload runtime state")
        .expect("runtime snapshot");
    assert_eq!(
        runtime.default_scene_zones[0].layout.zones[0].device_id,
        CANONICAL_LAYOUT_ID
    );
    let logical =
        crate::logical_devices::load_segments(&logical_path).expect("reload logical devices");
    assert_eq!(
        logical[CANONICAL_LAYOUT_ID].physical_device_id,
        canonical_physical_id
    );
    assert!(!logical.contains_key(LEGACY_LAYOUT_ID));
    let profiles = ComponentProfileStore::load(&profiles_path).expect("reload profiles");
    assert!(profiles.get(&canonical_physical_id.to_string()).is_some());
    assert!(profiles.get(&legacy_physical_id.to_string()).is_none());
    let settings = DeviceSettingsStore::load(&settings_path).expect("reload settings");
    assert!(
        settings
            .device_settings_for_key(CANONICAL_FINGERPRINT)
            .is_some()
    );
    assert!(
        settings
            .device_settings_for_key(LEGACY_FINGERPRINT)
            .is_none()
    );
    assert_eq!(
        settings
            .driver_control_values_for_key(&canonical_physical_id.to_string())
            .expect("canonical driver controls")
            .get("enabled"),
        Some(&ControlValue::Bool(true))
    );
    let preferences = DisplayPreferencesStore::load(&preferences_path).expect("reload preferences");
    assert!(preferences.get(canonical_physical_id).is_some());
    assert!(preferences.get(legacy_physical_id).is_none());

    let mut restart_evidence = PersistedBindingEvidence::default();
    restart_evidence.observe_layout(&layouts["saved"]);
    for scene in stored_scenes.list() {
        for zone in &scene.zones {
            restart_evidence.observe_layout(&zone.layout);
        }
    }
    assert!(
        build_binding_remaps(
            &restart_evidence,
            &[current_binding(canonical_physical_id)],
            &HashSet::from([CANONICAL_FINGERPRINT.to_owned()]),
        )
        .layout_device_ids
        .is_empty()
    );
}
