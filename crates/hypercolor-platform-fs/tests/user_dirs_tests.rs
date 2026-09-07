use std::path::Component;

use hypercolor_platform_fs::user_dirs;

#[test]
fn base_dirs_resolve_to_absolute_paths() {
    let config = user_dirs::config_base_dir().expect("config base resolves");
    let data = user_dirs::data_base_dir().expect("data base resolves");
    assert!(config.is_absolute(), "config base: {}", config.display());
    assert!(data.is_absolute(), "data base: {}", data.display());
}

#[test]
fn app_cache_dir_nests_the_application_segment() {
    let cache = user_dirs::app_cache_dir("hypercolor-user-dirs-test").expect("cache resolves");
    assert!(cache.is_absolute(), "cache: {}", cache.display());
    assert!(
        cache
            .components()
            .any(|component| component == Component::Normal("hypercolor-user-dirs-test".as_ref())),
        "cache dir lacks the app segment: {}",
        cache.display()
    );
    if cfg!(target_os = "linux") {
        assert_eq!(
            cache.file_name(),
            Some("hypercolor-user-dirs-test".as_ref())
        );
    } else {
        assert_eq!(cache.file_name(), Some("cache".as_ref()));
    }
}
