//! Emit the cross-language portable setup contract fixture.
use hypercolor_types::portable_setup::{
    PortableAsset, PortableAttachments, PortableBindingClass, PortableBindingHint, PortableBlob,
    PortableDevice, PortableDeviceOutput, PortableDisplayFace, PortableDisplayTarget,
    PortableEffect, PortableLayout, PortableLogicalDevice, PortableOutput, PortableOutputState,
    PortableReferenceCatalog, PortableScene, PortableSegmentShape, PortableSelection,
    PortableSetupV1, PortableZone,
};
use hypercolor_types::{
    asset::AssetId,
    attachment::{ComponentBinding, ComponentOrigin, ComponentSlot, ComponentTemplateManifest},
    effect::EffectId,
    layer::{BlendMode, LayerSource, MediaPlayback, SceneLayer, SceneLayerId},
    library::{
        EffectPlaylist, EffectPreset, PlaylistId, PlaylistItem, PlaylistItemId, PlaylistItemTarget,
        PresetId,
    },
    scene::{
        ColorInterpolation, DisplayRotation, EasingFunction, SceneId, SceneKind, SceneMutationMode,
        ScenePriority, TransitionSpec, UnassignedBehavior, ZoneId, ZoneRole,
    },
    spatial::{EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, StripDirection},
};
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

fn id(n: u64) -> Uuid {
    Uuid::from_u128(0x019a0000_0000_7000_8000_000000000000 | u128::from(n))
}

fn main() {
    let mut setup = PortableSetupV1::new(
        id(1),
        "Studio".into(),
        PortableOutputState {
            canvas_width: 640,
            canvas_height: 480,
            brightness: 0.8,
            enabled: true,
            paused: false,
        },
    );
    setup.devices.insert(
        id(2),
        PortableDevice {
            name: "Controller".into(),
            portable_key: None,
            driver_id: "lian_li".into(),
            vendor_id: Some(0x0cf2),
            product_id: Some(0xa101),
            model: Some("case".into()),
            led_count: 16,
            binding_hint: Some(PortableBindingHint {
                class: PortableBindingClass::ClaimlessUsb {
                    owner: "lian_li".into(),
                    vendor_id: 0x0cf2,
                    product_id: 0xa101,
                },
                segments: vec![PortableSegmentShape {
                    name: "strip".into(),
                    led_count: 16,
                }],
            }),
        },
    );
    setup.logical_devices.insert(
        id(3),
        PortableLogicalDevice {
            device: id(2),
            name: "Front strip".into(),
            led_start: 0,
            led_count: 16,
            enabled: true,
        },
    );
    let layout = PortableLayout {
        id: "case".into(),
        name: "Case".into(),
        description: None,
        zones: vec![PortableOutput {
            id: "front".into(),
            name: "Front".into(),
            device: id(3),
            zone_name: Some("strip".into()),
            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(0.4, 0.8),
            rotation: 0.0,
            scale: 1.0,
            display_order: 0,
            orientation: None,
            topology: LedTopology::Strip {
                count: 16,
                direction: StripDirection::LeftToRight,
            },
            led_mapping: None,
            sampling_mode: None,
            edge_behavior: None,
            shape: None,
            shape_preset: None,
            attachment: None,
            brightness: Some(0.75),
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    };
    setup.layouts.insert(layout.id.clone(), layout.clone());
    let effect_id = EffectId::new(id(4));
    let preset_id = PresetId(id(5));
    let effect_layer = SceneLayer::from_effect(
        SceneLayerId(id(6)),
        effect_id,
        HashMap::new(),
        HashMap::new(),
        Some(preset_id),
    );
    let mut media_layer = effect_layer.clone();
    media_layer.id = SceneLayerId(id(7));
    media_layer.source = LayerSource::Media {
        asset_id: AssetId(id(8)),
        playback: MediaPlayback::default(),
    };
    setup.scenes.insert(
        id(9),
        PortableScene {
            name: "Studio scene".into(),
            description: None,
            zones: vec![PortableZone {
                id: ZoneId(id(10)),
                name: "Primary".into(),
                description: None,
                layers: vec![effect_layer, media_layer],
                layout,
                brightness: 1.0,
                enabled: true,
                color: None,
                display_target: Some(PortableDisplayTarget {
                    device: id(2),
                    blend_mode: BlendMode::Alpha,
                    opacity: 1.0,
                }),
                role: ZoneRole::Primary,
            }],
            transition: TransitionSpec {
                duration_ms: 500,
                easing: EasingFunction::EaseInOut,
                color_interpolation: ColorInterpolation::Oklab,
            },
            priority: ScenePriority::USER,
            enabled: true,
            metadata: HashMap::new(),
            unassigned_behavior: UnassignedBehavior::Off,
            layout_id: Some(hypercolor_types::identity::LayoutId::new("case").expect("layout id")),
            activation_brightness: None,
            kind: SceneKind::Named,
            mutation_mode: SceneMutationMode::Live,
        },
    );
    setup.presets.insert(
        id(5),
        EffectPreset {
            id: preset_id,
            name: "Slow".into(),
            description: None,
            effect_id,
            controls: HashMap::new(),
            tags: Vec::new(),
            created_at_ms: 1,
            updated_at_ms: 1,
        },
    );
    setup.playlists.insert(
        id(11),
        EffectPlaylist {
            id: PlaylistId(id(11)),
            name: "Evening".into(),
            description: None,
            items: vec![PlaylistItem {
                id: PlaylistItemId(id(12)),
                target: PlaylistItemTarget::Preset { preset_id },
                duration_ms: Some(10000),
                transition_ms: Some(500),
            }],
            loop_enabled: true,
            created_at_ms: 1,
            updated_at_ms: 1,
        },
    );
    setup.assets.insert(
        id(8),
        PortableAsset {
            name: "Animation".into(),
            mime_type: "image/png".into(),
            tags: vec!["favorite".into()],
            content: PortableBlob {
                sha256: "a".repeat(64),
                byte_len: 1234,
            },
        },
    );
    setup.effects.insert(
        id(4),
        PortableEffect {
            name: "Custom effect".into(),
            entrypoint: "index.html".into(),
            files: BTreeMap::from([(
                "index.html".into(),
                PortableBlob {
                    sha256: "b".repeat(64),
                    byte_len: 4321,
                },
            )]),
        },
    );
    let mut template = ComponentTemplateManifest::default().template;
    template.id = "user-strip".into();
    template.name = "Strip".into();
    template.origin = ComponentOrigin::User;
    setup
        .attachment_templates
        .insert(template.id.clone(), template);
    setup.attachments.insert(
        id(2),
        PortableAttachments {
            slots: vec![ComponentSlot {
                id: "front".into(),
                name: "Front".into(),
                led_start: 0,
                led_count: 16,
                suggested_categories: Vec::new(),
                allowed_templates: vec!["user-strip".into()],
                allow_custom: true,
            }],
            bindings: vec![ComponentBinding {
                slot_id: "front".into(),
                template_id: "user-strip".into(),
                name: None,
                enabled: true,
                instances: 1,
                led_offset: 0,
            }],
        },
    );
    setup.display_faces.insert(
        id(2),
        PortableDisplayFace {
            effect_id,
            controls: HashMap::new(),
            blend_mode: BlendMode::Alpha,
            opacity: 1.0,
        },
    );
    setup.device_output.insert(
        id(2),
        PortableDeviceOutput {
            disabled: false,
            brightness: 0.7,
            rotation: DisplayRotation::default(),
        },
    );
    setup.selection = PortableSelection {
        active_scene_id: Some(SceneId(id(9))),
        default_scene_id: Some(SceneId(id(9))),
    };
    setup
        .validate_with_catalog(&PortableReferenceCatalog::default())
        .expect("valid fixture");
    println!(
        "{}",
        serde_json::to_string_pretty(&setup).expect("serialize fixture")
    );
}
