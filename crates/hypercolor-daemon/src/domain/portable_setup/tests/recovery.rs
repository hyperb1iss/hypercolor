use super::{Fixture, SetupBindings, options};
use crate::domain::portable_setup::journal::{
    BackupAncillary, RestoreRecovery, RuntimeRestoreProof,
};
use crate::persistence::PersistenceShutdown;
use hypercolor_types::portable_setup::PortablePreferences;
use uuid::Uuid;

#[tokio::test]
async fn backup_recovery_does_not_overwrite_newer_finite_preferences() {
    let fixture = Fixture::new("local-endpoint");
    let id = Uuid::now_v7();
    let before = fixture.empty(id).await;
    let original = fixture.config.get().as_ref().clone();
    let expected = PortablePreferences::from_config(&original);
    let mut target = original.clone();
    target.audio.smoothing = 0.61;
    let desired = PortablePreferences::from_config(&target);
    let mut changed = original;
    changed.audio.smoothing = 0.73;
    fixture.config.update(changed);
    let runtime = crate::runtime_state::snapshot_from_scene_manager(
        &fixture.state.scene_manager.snapshot().await,
    );
    let result = Box::pin(fixture.service.apply_candidate(
        before.manifest.clone(),
        before.manifest,
        "backup".into(),
        SetupBindings::default(),
        options(before.revision),
        &PersistenceShutdown::default(),
        Some(RestoreRecovery {
            runtime: RuntimeRestoreProof {
                before: None,
                candidate: runtime,
            },
            bindings: SetupBindings::default(),
        }),
        Some(BackupAncillary {
            preferences: Some(desired),
            expected_preferences: Some(expected),
            favorites: Vec::new(),
            tui_preferences: None,
            expected_tui_preferences: None,
        }),
    ))
    .await;
    assert!(
        result.is_err(),
        "restart must retain a preference outside the admitted before/candidate values"
    );
    assert_eq!(fixture.config.get().audio.smoothing, 0.73);
    assert_eq!(fixture.config.get().audio.device, "local-endpoint");
    assert!(fixture.service.read_journal(id).expect("journal").is_none());
}
