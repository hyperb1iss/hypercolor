use hypercolor_app::logging::{LOG_FILE_PREFIX, log_dir};

#[test]
fn log_dir_lives_under_hypercolor_logs() {
    let log_dir = log_dir();

    assert_eq!(
        log_dir.file_name().and_then(|name| name.to_str()),
        Some("logs")
    );
}

#[test]
fn app_log_file_prefix_is_stable() {
    assert_eq!(LOG_FILE_PREFIX, "hypercolor-app.log");
}
