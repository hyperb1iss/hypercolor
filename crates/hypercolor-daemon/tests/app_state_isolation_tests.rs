//! The daemon library is built without `cfg(test)` for this integration test.

use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::app_state::AppState;

#[test]
fn empty_state_never_uses_ambient_user_storage() {
    // Keep even the regressed implementation away from the developer's files.
    // This binary has one test, so these process-wide overrides have one owner.
    let ambient = tempfile::tempdir().expect("ambient storage fixture creates");
    let data = ambient.path().join("data");
    let state = ambient.path().join("state");
    ConfigManager::set_data_dir_override(Some(data.clone()));
    ConfigManager::set_state_dir_override(Some(state.clone()));
    ConfigManager::set_config_dir_override(Some(ambient.path().join("config")));

    let first = AppState::new();
    let second = AppState::default();
    let third = AppState::builder().build();
    for fixture in [&first, &second, &third] {
        assert_ne!(fixture.data_dir, data);
        assert_ne!(fixture.state_dir, state);
        assert!(fixture.state_dir.starts_with(&fixture.data_dir));
        assert!(fixture.data_dir.is_dir());
    }
    assert_ne!(first.data_dir, second.data_dir);
    assert_ne!(second.data_dir, third.data_dir);
    assert!(!data.exists());
    assert!(!state.exists());

    let temporary = first.data_dir.clone();
    drop(first);
    assert!(!temporary.exists());

    let explicit = ambient.path().join("explicit");
    let fixture = AppState::new_with_data_dir(explicit.clone());
    assert_eq!(fixture.data_dir, explicit);
    assert_eq!(fixture.state_dir, explicit.join("state"));
    drop(fixture);
    assert!(explicit.exists());

    ConfigManager::set_data_dir_override(None);
    ConfigManager::set_state_dir_override(None);
    ConfigManager::set_config_dir_override(None);
}
