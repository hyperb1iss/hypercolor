use std::collections::HashMap;
use std::str::FromStr;

use hypercolor_color::PixelBlendMode;
use hypercolor_types::asset::AssetId;
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::{ControlBinding, EffectId};
use hypercolor_types::layer::{
    AudioBand, BindingMap, BindingSource, BlendMode, LayerAdjust, LayerBinding, LayerParameter,
    LayerScope, LayerSource, LayerTransform, LoopMode, MediaPlayback, SceneLayer, SceneLayerId,
    TimeWave, WebViewportRender,
};
use hypercolor_types::viewport::FitMode;
use uuid::Uuid;

#[test]
fn authored_blend_modes_have_one_wire_vocabulary() {
    let modes = [
        BlendMode::Replace,
        BlendMode::Alpha,
        BlendMode::Tint,
        BlendMode::LumaReveal,
        BlendMode::Add,
        BlendMode::Screen,
        BlendMode::Multiply,
        BlendMode::Overlay,
        BlendMode::SoftLight,
        BlendMode::ColorDodge,
        BlendMode::Difference,
    ];

    for mode in modes {
        let json = serde_json::to_string(&mode).expect("serialize blend mode");
        let restored = serde_json::from_str(&json).expect("deserialize blend mode");
        assert_eq!(mode, restored);
    }

    assert_eq!(BlendMode::default(), BlendMode::Alpha);
    assert!(!BlendMode::Replace.blends_with_base());
    assert_eq!(BlendMode::Alpha.pixel_mode(), Some(PixelBlendMode::Normal));
    assert_eq!(BlendMode::Tint.pixel_mode(), None);
}

#[test]
fn scene_layer_id_round_trips_through_uuid_and_display() {
    let uuid = Uuid::now_v7();
    let id = SceneLayerId::from_uuid(uuid);

    assert_eq!(id.as_uuid(), uuid);
    assert_eq!(id.to_string(), uuid.to_string());
    assert_eq!(
        SceneLayerId::from_str(&id.to_string()).expect("valid layer id"),
        id
    );
}

#[test]
fn effect_layer_round_trips_through_json() {
    let effect_id = EffectId::from(Uuid::now_v7());
    let layer = SceneLayer::from_effect(
        SceneLayerId::new(),
        effect_id,
        HashMap::from([("speed".to_owned(), ControlValue::Float(1.25))]),
        HashMap::from([(
            "speed".to_owned(),
            ControlBinding {
                sensor: "gpu_temp".to_owned(),
                sensor_min: 40.0,
                sensor_max: 85.0,
                target_min: 0.25,
                target_max: 1.5,
                deadband: 0.0,
                smoothing: 0.2,
            },
        )]),
        None,
    );

    let json = serde_json::to_string(&layer).expect("serialize layer");
    let restored: SceneLayer = serde_json::from_str(&json).expect("deserialize layer");

    assert_eq!(restored, layer);
    assert_eq!(restored.blend, BlendMode::Replace);
}

#[test]
fn media_layer_defaults_playback_transform_and_adjust() {
    let json = serde_json::json!({
        "id": SceneLayerId::new(),
        "source": {
            "type": "media",
            "asset_id": AssetId::new()
        }
    });

    let layer: SceneLayer = serde_json::from_value(json).expect("deserialize media layer");

    assert!(layer.enabled);
    assert_eq!(layer.opacity, 1.0);
    assert_eq!(layer.blend, BlendMode::Alpha);
    assert_eq!(layer.transform.fit, FitMode::Cover);
    assert_eq!(layer.adjust, LayerAdjust::default());
    let LayerSource::Media { playback, .. } = layer.source else {
        panic!("expected media source");
    };
    assert_eq!(playback, MediaPlayback::default());
}

#[test]
fn web_viewport_defaults_to_live_full_viewport() {
    let json = serde_json::json!({
        "id": SceneLayerId::new(),
        "source": {
            "type": "web_viewport",
            "url": "https://example.com"
        }
    });

    let layer: SceneLayer = serde_json::from_value(json).expect("deserialize web viewport layer");
    let LayerSource::WebViewport {
        viewport, render, ..
    } = layer.source
    else {
        panic!("expected web viewport source");
    };

    assert_eq!(viewport.x, 0.0);
    assert_eq!(viewport.width, 1.0);
    assert_eq!(render, WebViewportRender::Live);
}

#[test]
fn layer_normalization_clamps_runtime_safe_scalars() {
    let layer = SceneLayer {
        id: SceneLayerId::new(),
        name: None,
        source: LayerSource::ColorFill {
            rgba: [1.0, 1.0, 1.0, 1.0],
        },
        blend: BlendMode::Alpha,
        opacity: 12.0,
        transform: LayerTransform {
            scale: [0.0, 99.0],
            rotation: f32::NAN,
            ..LayerTransform::default()
        },
        adjust: LayerAdjust {
            brightness: f32::NAN,
            saturation: 9.0,
            tint_strength: 2.0,
            contrast: -3.0,
            ..LayerAdjust::default()
        },
        bindings: Vec::new(),
        enabled: true,
        scope: None,
    };

    let normalized = layer.normalized();

    assert_eq!(normalized.opacity, 1.0);
    assert_eq!(normalized.transform.scale, [0.01, 16.0]);
    assert_eq!(normalized.transform.rotation, 0.0);
    assert_eq!(normalized.adjust.brightness, 1.0);
    assert_eq!(normalized.adjust.saturation, 4.0);
    assert_eq!(normalized.adjust.tint_strength, 1.0);
    assert_eq!(normalized.adjust.contrast, -1.0);
}

#[test]
fn layer_validation_rejects_non_finite_values_and_empty_binding_ranges() {
    let layer = SceneLayer {
        id: SceneLayerId::new(),
        name: None,
        source: LayerSource::ColorFill {
            rgba: [1.0, 1.0, 1.0, 1.0],
        },
        blend: BlendMode::Alpha,
        opacity: f32::NAN,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: vec![LayerBinding {
            target: LayerParameter::Opacity,
            source: BindingSource::AudioBand {
                band: AudioBand::Bass,
            },
            map: BindingMap {
                source_min: 1.0,
                source_max: 1.0,
                target_min: 0.0,
                target_max: 1.0,
                clamp: true,
            },
        }],
        enabled: true,
        scope: None,
    };

    let errors = layer.validate().expect_err("invalid layer should fail");

    assert!(errors.iter().any(|error| error.contains("opacity")));
    assert!(
        errors
            .iter()
            .any(|error| error.contains("source range must not be empty"))
    );
}

#[test]
fn layer_validation_rejects_out_of_range_scalars() {
    let layer = SceneLayer {
        id: SceneLayerId::new(),
        name: None,
        source: LayerSource::ColorFill {
            rgba: [1.5, 1.0, 1.0, 1.0],
        },
        blend: BlendMode::Alpha,
        opacity: 1.0,
        transform: LayerTransform {
            scale: [0.001, 20.0],
            ..LayerTransform::default()
        },
        adjust: LayerAdjust {
            brightness: 5.0,
            tint_strength: -0.25,
            ..LayerAdjust::default()
        },
        bindings: Vec::new(),
        enabled: true,
        scope: None,
    };

    let errors = layer
        .validate()
        .expect_err("out-of-range layer should fail");

    assert!(errors.iter().any(|error| error.contains("source.rgba[0]")));
    assert!(
        errors
            .iter()
            .any(|error| error.contains("transform.scale[0]"))
    );
    assert!(
        errors
            .iter()
            .any(|error| error.contains("adjust.brightness"))
    );
}

#[test]
fn binding_map_linear_records_ranges() {
    let map = BindingMap::linear(-1.0..=1.0, 0.25..=0.75);

    assert_eq!(map.source_min, -1.0);
    assert_eq!(map.source_max, 1.0);
    assert_eq!(map.target_min, 0.25);
    assert_eq!(map.target_max, 0.75);
    assert!(map.clamp);
}

#[test]
fn playback_and_binding_enums_serialize_as_snake_case() {
    let playback = MediaPlayback {
        speed: 0.5,
        loop_mode: LoopMode::PingPong,
        start_offset_secs: 1.0,
        auto_play: true,
    };
    let source = BindingSource::Time {
        rate_hz: 0.25,
        wave: TimeWave::Triangle,
    };

    let playback_json = serde_json::to_value(playback).expect("serialize playback");
    let source_json = serde_json::to_value(source).expect("serialize source");

    assert_eq!(playback_json["loop_mode"], "ping_pong");
    assert_eq!(source_json["wave"], "triangle");
}

fn scoped_layer(outputs: &[&str]) -> SceneLayer {
    let mut layer = SceneLayer::from_effect(
        SceneLayerId::new(),
        EffectId::from(Uuid::now_v7()),
        HashMap::new(),
        HashMap::new(),
        None,
    );
    layer.scope = Some(LayerScope {
        outputs: outputs.iter().map(|id| (*id).to_owned()).collect(),
    });
    layer
}

#[test]
fn a_layer_without_a_scope_serializes_without_the_field() {
    let layer = SceneLayer::from_effect(
        SceneLayerId::new(),
        EffectId::from(Uuid::now_v7()),
        HashMap::new(),
        HashMap::new(),
        None,
    );

    assert_eq!(layer.scope, None);
    let json = serde_json::to_value(&layer).expect("serialize layer");
    assert!(
        json.as_object()
            .expect("layer serializes as an object")
            .get("scope")
            .is_none(),
        "a whole-zone layer must not gain a scope key: {json}"
    );
}

#[test]
fn a_scoped_layer_round_trips_its_output_ids() {
    let layer = scoped_layer(&["usb:keyboard-main", "usb:push2-pads"]);

    let json = serde_json::to_value(&layer).expect("serialize layer");
    assert_eq!(
        json["scope"],
        serde_json::json!({ "outputs": ["usb:keyboard-main", "usb:push2-pads"] })
    );
    let restored: SceneLayer = serde_json::from_value(json).expect("deserialize layer");
    assert_eq!(restored, layer);
}

#[test]
fn a_layer_document_written_before_scope_existed_decodes_unchanged() {
    let stored = serde_json::json!({
        "id": "0195e5b0-b2ea-7f22-9ab2-9bc31b48adf3",
        "name": "Ripples",
        "source": {
            "type": "effect",
            "effect_id": "0195e5b0-b2ea-7f22-9ab2-9bc31b48adf4",
            "controls": {}
        },
        "blend": "screen",
        "opacity": 0.5,
        "transform": {
            "anchor": { "x": 0.5, "y": 0.5 },
            "scale": [1.0, 1.0],
            "rotation": 0.0,
            "fit": "cover"
        },
        "adjust": {
            "brightness": 1.0,
            "saturation": 1.0,
            "hue_shift": 0.0,
            "tint": [1.0, 1.0, 1.0, 1.0],
            "tint_strength": 0.0,
            "contrast": 0.0
        },
        "enabled": true
    });

    let layer: SceneLayer = serde_json::from_value(stored.clone()).expect("stored layer decodes");

    assert_eq!(layer.scope, None);
    assert_eq!(
        serde_json::to_value(&layer).expect("serialize layer"),
        stored,
        "decoding and re-encoding a pre-scope layer must not change it"
    );
}

#[test]
fn layer_normalization_sorts_and_dedupes_scope_outputs() {
    let layer = scoped_layer(&["usb:push2-pads", "usb:keyboard-main", "usb:push2-pads"]);

    let normalized = layer.normalized();

    assert_eq!(
        normalized.scope,
        Some(LayerScope {
            outputs: vec!["usb:keyboard-main".to_owned(), "usb:push2-pads".to_owned()],
        })
    );
    assert_eq!(normalized.normalized(), normalized);
}

#[test]
fn layer_validation_rejects_an_empty_scope() {
    let errors = scoped_layer(&[])
        .validate()
        .expect_err("a scope with no outputs is not a state");

    assert!(
        errors
            .iter()
            .any(|error| error.contains("scope.outputs must name at least one output")),
        "{errors:?}"
    );
}

#[test]
fn layer_validation_rejects_an_empty_scope_output_id() {
    let errors = scoped_layer(&["usb:keyboard-main", ""])
        .validate()
        .expect_err("an empty output id names nothing");

    assert!(
        errors
            .iter()
            .any(|error| error.contains("must not hold an empty output id")),
        "{errors:?}"
    );
}

#[test]
fn layer_validation_does_not_check_scope_membership() {
    scoped_layer(&["an-output-no-zone-holds"])
        .validate()
        .expect("membership needs the zone, so the layer alone validates");
}

#[test]
fn layer_scope_contains_matches_whole_ids_only() {
    let scope = LayerScope {
        outputs: vec!["usb:keyboard-main".to_owned()],
    };

    assert!(scope.contains("usb:keyboard-main"));
    assert!(!scope.contains("usb:keyboard"));
    assert!(!scope.contains(""));
}

// ── A zone's effect ──────────────────────────────────────────────────────

fn effect_layer_scoped_to(outputs: Option<&[&str]>) -> SceneLayer {
    let mut layer = SceneLayer::from_effect(
        SceneLayerId::new(),
        EffectId::new(Uuid::now_v7()),
        HashMap::new(),
        HashMap::new(),
        None,
    );
    layer.scope = outputs.map(|outputs| hypercolor_types::layer::LayerScope {
        outputs: outputs.iter().map(|id| (*id).to_owned()).collect(),
    });
    layer
}

#[test]
fn a_zones_effect_is_its_top_effect_layer_with_no_scope() {
    use hypercolor_types::layer::zone_effect_layer;

    let base = effect_layer_scoped_to(None);
    let upper = effect_layer_scoped_to(None);
    let fill = SceneLayer {
        source: LayerSource::ColorFill {
            rgba: [1.0, 0.0, 0.0, 1.0],
        },
        ..effect_layer_scoped_to(None)
    };
    let face = effect_layer_scoped_to(Some(&["pump-display"]));
    let keys = effect_layer_scoped_to(Some(&["keyboard"]));

    assert_eq!(
        zone_effect_layer(&[base.clone(), face.clone()]).map(|layer| layer.id),
        Some(base.id),
        "a screen's face on top of the stack is not the zone's effect"
    );
    assert_eq!(
        zone_effect_layer(&[base.clone(), keys.clone()]).map(|layer| layer.id),
        Some(base.id),
        "nor is a layer scoped to a keyboard"
    );
    assert_eq!(
        zone_effect_layer(&[
            base.clone(),
            upper.clone(),
            fill,
            keys.clone(),
            face.clone()
        ])
        .map(|layer| layer.id),
        Some(upper.id),
        "the topmost whole-zone effect wins, above it only overlays and fills"
    );
    assert!(
        zone_effect_layer(&[keys.clone(), face.clone()]).is_none(),
        "a zone with only scoped layers has no effect of its own"
    );
    assert!(zone_effect_layer(&[]).is_none());

    let mut stack = vec![base.clone(), face];
    assert_eq!(
        hypercolor_types::layer::zone_effect_layer_mut(&mut stack).map(|layer| layer.id),
        Some(base.id)
    );
    assert_eq!(base.effect_id(), stack[0].effect_id());
}
