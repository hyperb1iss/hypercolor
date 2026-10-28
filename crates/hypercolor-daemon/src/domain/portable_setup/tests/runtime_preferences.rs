use super::*;
use hypercolor_types::portable_setup::PortablePreferences;

#[tokio::test]
async fn restored_processing_preferences_reconfigure_the_live_input_graph() {
    let fixture = Fixture::new("none");
    fixture.config.modify(|config| {
        config.audio.enabled = true;
        config.audio.smoothing = 0.34;
        config.capture.enabled = false;
        config.capture.saturation = 1.4;
    });
    let expected = PortablePreferences::from_config(&fixture.config.get());
    let before = fixture.state.input_manager().source_graph_generation();
    fixture
        .service
        .reconcile_runtime_preferences(&expected)
        .await
        .expect("live preferences");
    assert!(fixture.state.input_manager().source_graph_generation() > before);
    assert_eq!(
        fixture.state.input_manager().source_names(),
        vec!["AudioInput(none)"]
    );
    let current = Arc::clone(&fixture.config.get());
    assert!(crate::api::config::live::capture_runtime_matches(&fixture.state, &current).await);
    assert_eq!(current.audio.device, "none");
    assert_eq!(current.audio.smoothing.to_bits(), 0.34_f32.to_bits());
}

#[tokio::test]
async fn newer_processing_preferences_are_not_overwritten_by_commit_recovery() {
    let fixture = Fixture::new("none");
    let old = PortablePreferences::from_config(&fixture.config.get());
    fixture
        .config
        .modify(|config| config.audio.smoothing = 0.23);
    let generation = fixture.state.input_manager().source_graph_generation();
    assert!(
        fixture
            .service
            .reconcile_runtime_preferences(&old)
            .await
            .is_err()
    );
    assert_eq!(
        fixture.state.input_manager().source_graph_generation(),
        generation
    );
    assert_eq!(
        fixture.config.get().audio.smoothing.to_bits(),
        0.23_f32.to_bits()
    );
}
