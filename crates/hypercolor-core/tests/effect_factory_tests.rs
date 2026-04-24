//! Tests for effect renderer factory routing.

use std::path::PathBuf;

use hypercolor_core::effect::{
    create_renderer_for_metadata, create_renderer_for_metadata_with_effect_acceleration,
    resolve_effect_renderer_acceleration_mode,
};
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use uuid::Uuid;

fn native_metadata(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "hypercolor".to_owned(),
        version: "0.1.0".to_owned(),
        description: "native test effect".to_owned(),
        category: EffectCategory::Ambient,
        tags: vec!["native".to_owned()],
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from(format!("builtin/{name}")),
        },
        license: None,
    }
}

fn html_metadata() -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: "aurora-html".to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: "html test effect".to_owned(),
        category: EffectCategory::Ambient,
        tags: vec!["html".to_owned()],
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Html {
            path: PathBuf::from("community/aurora.html"),
        },
        license: None,
    }
}

#[test]
fn factory_creates_renderer_for_builtin_native() {
    let mut metadata = native_metadata("rainbow");
    metadata.name = "Rainbow".to_owned();
    let renderer = create_renderer_for_metadata(&metadata);
    assert!(renderer.is_ok());
}

#[test]
fn factory_errors_for_unknown_native_renderer() {
    let Err(error) = create_renderer_for_metadata(&native_metadata("does-not-exist")) else {
        panic!("unknown native renderer should error");
    };

    assert!(
        error
            .to_string()
            .contains("has no built-in renderer implementation")
    );
}

#[test]
fn auto_render_acceleration_falls_back_to_cpu() {
    let resolution = resolve_effect_renderer_acceleration_mode(RenderAccelerationMode::Auto)
        .expect("auto mode should resolve");
    assert_eq!(resolution.effective_mode, RenderAccelerationMode::Cpu);
    assert!(resolution.fallback_reason.is_some());

    let renderer = create_renderer_for_metadata_with_effect_acceleration(
        &native_metadata("rainbow"),
        RenderAccelerationMode::Auto,
    );
    assert!(renderer.is_ok());
}

#[test]
fn gpu_render_acceleration_requires_a_real_gpu_lane() {
    let Err(error) = create_renderer_for_metadata_with_effect_acceleration(
        &native_metadata("rainbow"),
        RenderAccelerationMode::Gpu,
    ) else {
        panic!("gpu mode should error until the GPU lane exists");
    };

    assert!(
        error
            .to_string()
            .contains("gpu effect renderer acceleration is not available yet")
    );
}

#[cfg(not(feature = "servo"))]
#[test]
fn factory_html_requires_servo_feature() {
    let Err(error) = create_renderer_for_metadata(&html_metadata()) else {
        panic!("html should require servo");
    };

    assert!(error.to_string().contains("requires the `servo` feature"));
}

#[cfg(feature = "servo")]
#[test]
fn factory_creates_servo_renderer_for_html() {
    let renderer = create_renderer_for_metadata(&html_metadata());
    assert!(renderer.is_ok());
}
