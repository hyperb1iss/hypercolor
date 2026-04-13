#[path = "support/effect_engine.rs"]
mod effect_engine;

use hypercolor_core::device::mock::MockEffectRenderer;
use hypercolor_types::effect::ControlValue;

use effect_engine::EffectEngine;

#[test]
fn scene_generation_advances_on_effect_state_changes() {
    let mut engine = EffectEngine::new();
    assert_eq!(engine.scene_generation(), 0);

    engine
        .activate(
            Box::new(MockEffectRenderer::solid(255, 0, 0)),
            MockEffectRenderer::sample_metadata("generation"),
        )
        .expect("mock effect should activate");
    let activated_generation = engine.scene_generation();
    assert!(activated_generation > 0);

    engine.set_control("speed", &ControlValue::Float(0.5));
    let controlled_generation = engine.scene_generation();
    assert!(controlled_generation > activated_generation);

    engine.pause();
    let paused_generation = engine.scene_generation();
    assert!(paused_generation > controlled_generation);

    engine.resume();
    let resumed_generation = engine.scene_generation();
    assert!(resumed_generation > paused_generation);

    engine.deactivate();
    assert!(engine.scene_generation() > resumed_generation);
}
