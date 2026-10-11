use hypercolor_types::api::updates::{ReleaseUpdate, UpdateStatus};
use hypercolor_ui::release_notifications::{is_release_notification_event, should_notify};

fn status(version: Option<&str>, skipped: Option<&str>) -> UpdateStatus {
    UpdateStatus {
        current_version: "0.6.3".to_owned(),
        automatic_checks: true,
        skipped_version: skipped.map(str::to_owned),
        last_checked: None,
        release: version.map(|version| ReleaseUpdate {
            version: version.to_owned(),
            name: format!("Hypercolor {version}"),
            notes: String::new(),
            release_url: format!(
                "https://github.com/hyperb1iss/hypercolor/releases/tag/v{version}"
            ),
            download_url: None,
        }),
    }
}

#[test]
fn notification_skip_applies_only_to_the_chosen_release() {
    assert!(should_notify(&status(Some("0.7.0"), None)));
    assert!(!should_notify(&status(Some("0.7.0"), Some("0.7.0"))));
    assert!(should_notify(&status(Some("0.7.1"), Some("0.7.0"))));
    assert!(!should_notify(&status(None, Some("0.7.0"))));
    assert!(!should_notify(&status(None, None)));
}

#[test]
fn refetch_hints_require_the_release_extension_and_status_kind() {
    assert!(is_release_notification_event("oss-updates", "status"));
    assert!(!is_release_notification_event("cloud", "status"));
    assert!(!is_release_notification_event("oss-updates", "progress"));
}
