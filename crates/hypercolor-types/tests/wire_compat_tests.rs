//! Wire-format compatibility tests.
//!
//! These tests are the gate for Plan 55 Wave P3 (the codebase-wide
//! type rename `RenderGroup`→`Zone`, `DeviceZone`→`Output`,
//! `Attachment*`→`Component*`). The rename is a Rust-identifier
//! rename only — the serialized wire format must stay byte-identical.
//! These tests build a representative Scene and AttachmentTemplate
//! with the affected types populated, serialize them, and assert the
//! bytes match a checked-in golden. Run as part of `just verify`; a
//! diff means an unintentional wire change crept in.
//!
//! Regenerating the goldens (only when a wire change is genuinely
//! intentional):
//!
//! ```ignore
//! BOOTSTRAP_FIXTURES=1 cargo test -p hypercolor-types --test wire_compat_tests
//! ```
//!
//! HashMap iteration order is non-deterministic; the fixture data
//! keeps every `HashMap`-typed field empty so the golden stays
//! reproducible across runs.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use uuid::{Uuid, uuid};

use hypercolor_types::attachment::{
    AttachmentCanvasSize, AttachmentCategory, AttachmentCompatibility, AttachmentOrigin,
    AttachmentTemplate, AttachmentTemplateManifest,
};
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::{SceneLayer, SceneLayerId};
use hypercolor_types::scene::{
    ColorInterpolation, EasingFunction, RenderGroup, RenderGroupId, RenderGroupRole, Scene,
    SceneId, SceneKind, SceneMutationMode, ScenePriority, SceneScope, TransitionSpec,
    UnassignedBehavior,
};
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection, ZoneAttachment, ZoneShape,
};

const SCENE_FIXTURE: &str = "tests/fixtures/wire_compat_scene.json";
const ATTACHMENT_FIXTURE: &str = "tests/fixtures/wire_compat_attachment_template.toml";

const FIXTURE_SCENE_UUID: Uuid = uuid!("01931234-5678-7abc-9def-0123456789ab");
const FIXTURE_GROUP_UUID: Uuid = uuid!("01931234-5678-7def-9abc-0123456789cd");
const FIXTURE_EFFECT_UUID: Uuid = uuid!("01931234-5678-7111-9222-0123456789ef");
const FIXTURE_LAYER_UUID: Uuid = uuid!("01931234-5678-7333-9444-0123456789aa");

fn fixture_path(suffix: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push(suffix);
    path
}

/// Deterministic Scene that exercises every rename-affected type.
fn build_fixture_scene() -> Scene {
    let layout = SpatialLayout {
        id: "layout-fixture".to_owned(),
        name: "Fixture layout".to_owned(),
        description: Some("Wire-compat layout snapshot".to_owned()),
        canvas_width: 640,
        canvas_height: 480,
        zones: vec![DeviceZone {
            id: "output-strip-1".to_owned(),
            name: "Front strip".to_owned(),
            device_id: "usb:controller-1".to_owned(),
            zone_name: Some("channel-a".to_owned()),
            position: NormalizedPosition::new(0.25, 0.5),
            size: NormalizedPosition::new(0.4, 0.1),
            rotation: 0.0,
            scale: 1.0,
            display_order: 0,
            orientation: None,
            topology: LedTopology::Strip {
                count: 24,
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: Some(SamplingMode::Bilinear),
            edge_behavior: Some(EdgeBehavior::Clamp),
            shape: Some(ZoneShape::Rectangle),
            shape_preset: Some("strip-24".to_owned()),
            attachment: Some(ZoneAttachment {
                template_id: "lianli-strimer-24pin".to_owned(),
                slot_id: "atx".to_owned(),
                instance: 0,
                led_start: Some(0),
                led_count: Some(24),
                led_mapping: None,
            }),
            brightness: Some(0.8),
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    let group = RenderGroup {
        id: RenderGroupId(FIXTURE_GROUP_UUID),
        name: "Default zone".to_owned(),
        description: Some("Fixture render group".to_owned()),
        effect_id: Some(EffectId(FIXTURE_EFFECT_UUID)),
        controls: HashMap::new(),
        control_bindings: HashMap::new(),
        preset_id: None,
        layers: vec![SceneLayer::from_effect(
            SceneLayerId::from_uuid(FIXTURE_LAYER_UUID),
            EffectId(FIXTURE_EFFECT_UUID),
            HashMap::new(),
            HashMap::new(),
            None,
        )],
        layout,
        brightness: 1.0,
        enabled: true,
        color: Some("#80ffea".to_owned()),
        display_target: None,
        role: RenderGroupRole::Primary,
        controls_version: 0,
        layers_version: 0,
    };

    Scene {
        id: SceneId(FIXTURE_SCENE_UUID),
        name: "Wire compat scene".to_owned(),
        description: Some("Locks the wire format Plan 55 P3 must preserve".to_owned()),
        scope: SceneScope::Full,
        zone_assignments: Vec::new(),
        groups: vec![group],
        groups_revision: 7,
        transition: TransitionSpec {
            duration_ms: 750,
            easing: EasingFunction::EaseInOut,
            color_interpolation: ColorInterpolation::Oklab,
        },
        priority: ScenePriority::USER,
        enabled: true,
        metadata: HashMap::new(),
        unassigned_behavior: UnassignedBehavior::Off,
        kind: SceneKind::Named,
        mutation_mode: SceneMutationMode::Live,
    }
}

/// Deterministic AttachmentTemplateManifest that exercises every
/// rename-affected `Attachment*` type — including the hand-written
/// `Serialize` on `AttachmentCategory`.
fn build_fixture_attachment_manifest() -> AttachmentTemplateManifest {
    AttachmentTemplateManifest {
        schema_version: 1,
        template: AttachmentTemplate {
            id: "fixture-strip-24".to_owned(),
            name: "Fixture 24-LED strip".to_owned(),
            vendor: "fixtureco".to_owned(),
            category: AttachmentCategory::Strip,
            description: "Wire-compat strip template".to_owned(),
            tags: vec!["strip".to_owned(), "fixture".to_owned()],
            origin: AttachmentOrigin::BuiltIn,
            topology: LedTopology::Strip {
                count: 24,
                direction: StripDirection::LeftToRight,
            },
            default_size: AttachmentCanvasSize {
                width: 0.4,
                height: 0.1,
            },
            compatible_slots: vec![AttachmentCompatibility {
                controller_ids: vec!["lianli".to_owned()],
                models: vec!["strimer-plus-24pin".to_owned()],
                slots: vec!["atx".to_owned()],
            }],
            led_names: None,
            led_mapping: None,
            image_url: None,
            physical_size_mm: Some((300.0, 24.0)),
        },
    }
}

#[test]
fn scene_wire_format_matches_golden() {
    let scene = build_fixture_scene();
    let serialized =
        serde_json::to_string_pretty(&scene).expect("Scene serializes to JSON cleanly");

    let path = fixture_path(SCENE_FIXTURE);
    if std::env::var_os("BOOTSTRAP_FIXTURES").is_some() {
        fs::create_dir_all(path.parent().expect("fixture has parent dir"))
            .expect("create fixtures dir");
        fs::write(&path, format!("{serialized}\n")).expect("write scene fixture");
        return;
    }

    let golden = fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "Scene wire-compat golden missing at {}. Seed it with \
             BOOTSTRAP_FIXTURES=1 cargo test -p hypercolor-types \
             --test wire_compat_tests",
            path.display()
        );
    });
    assert_eq!(
        serialized.trim(),
        golden.trim(),
        "Scene wire format diverged from golden. If intentional, regenerate \
         with BOOTSTRAP_FIXTURES=1 cargo test -p hypercolor-types --test \
         wire_compat_tests."
    );

    let parsed: Scene = serde_json::from_str(&golden).expect("golden scene parses back");
    let reserialized =
        serde_json::to_string_pretty(&parsed).expect("Parsed scene re-serializes cleanly");
    assert_eq!(
        reserialized.trim(),
        golden.trim(),
        "Scene golden did not round-trip — serde is asymmetric"
    );
}

#[test]
fn attachment_template_manifest_wire_format_matches_golden() {
    let manifest = build_fixture_attachment_manifest();
    let serialized =
        toml::to_string_pretty(&manifest).expect("AttachmentTemplateManifest serializes to TOML");

    let path = fixture_path(ATTACHMENT_FIXTURE);
    if std::env::var_os("BOOTSTRAP_FIXTURES").is_some() {
        fs::create_dir_all(path.parent().expect("fixture has parent dir"))
            .expect("create fixtures dir");
        fs::write(&path, &serialized).expect("write attachment fixture");
        return;
    }

    let golden = fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "AttachmentTemplate wire-compat golden missing at {}. Seed \
             it with BOOTSTRAP_FIXTURES=1 cargo test -p hypercolor-types \
             --test wire_compat_tests",
            path.display()
        );
    });
    assert_eq!(
        serialized.trim(),
        golden.trim(),
        "AttachmentTemplate wire format diverged from golden. If \
         intentional, regenerate with BOOTSTRAP_FIXTURES=1 cargo test \
         -p hypercolor-types --test wire_compat_tests."
    );

    let parsed: AttachmentTemplateManifest =
        toml::from_str(&golden).expect("golden attachment manifest parses back");
    let reserialized = toml::to_string_pretty(&parsed)
        .expect("Parsed AttachmentTemplateManifest re-serializes cleanly");
    assert_eq!(
        reserialized.trim(),
        golden.trim(),
        "AttachmentTemplate golden did not round-trip"
    );
}
