use std::collections::HashMap;
use std::str::FromStr;

use hypercolor_types::asset::AssetId;
use hypercolor_types::effect::{ControlBinding, ControlValue, EffectId};
use hypercolor_types::layer::{
    AudioBand, BindingMap, BindingSource, LayerAdjust, LayerBinding, LayerBlendMode,
    LayerParameter, LayerSource, LayerTransform, LoopMode, MAX_LAYER_BINDINGS, MediaPlayback,
    SceneLayer, SceneLayerId, TimeWave, WebViewportRender,
};
use hypercolor_types::viewport::FitMode;
use uuid::Uuid;

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
    assert_eq!(restored.blend, LayerBlendMode::Replace);
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
    assert_eq!(layer.blend, LayerBlendMode::Alpha);
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
        blend: LayerBlendMode::Alpha,
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
        blend: LayerBlendMode::Alpha,
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
fn layer_validation_rejects_too_many_bindings() {
    let layer = SceneLayer {
        id: SceneLayerId::new(),
        name: None,
        source: LayerSource::ColorFill {
            rgba: [1.0, 1.0, 1.0, 1.0],
        },
        blend: LayerBlendMode::Alpha,
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: (0..=MAX_LAYER_BINDINGS)
            .map(|_| LayerBinding {
                target: LayerParameter::Opacity,
                source: BindingSource::AudioBand {
                    band: AudioBand::Bass,
                },
                map: BindingMap::linear(0.0..=1.0, 0.0..=1.0),
            })
            .collect(),
        enabled: true,
    };

    let errors = layer
        .validate()
        .expect_err("layer with too many bindings should fail");

    assert!(
        errors
            .iter()
            .any(|error| error.contains("bindings must contain at most"))
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
        blend: LayerBlendMode::Alpha,
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
