use std::fs;

use hypercolor_core::config::servers::{StoredServersError, load_server_credentials};
use tempfile::tempdir;

#[test]
fn shared_server_parser_validates_and_preserves_endpoint_binding() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("servers.toml");
    fs::write(
        &path,
        r#"
            [[servers]]
            instance_id = " daemon-a "
            api_key = " secret-a "
            host = "127.0.0.1"
            port = 9420

            [[servers]]
            instance_id = "daemon-b"
            api_key = "secret-b"

            [[servers]]
            instance_id = " "
            api_key = "ignored"
        "#,
    )
    .expect("fixture writes");

    let credentials = load_server_credentials(&path).expect("fixture parses");
    assert_eq!(credentials.len(), 2);
    assert_eq!(credentials[0].instance_id(), "daemon-a");
    assert_eq!(credentials[0].api_key(), "secret-a");
    assert_eq!(
        credentials[0].endpoint(),
        Some(("127.0.0.1".parse().expect("valid ip"), 9420))
    );
    assert_eq!(credentials[1].endpoint(), None);
}

#[test]
fn shared_server_parser_redacts_credentials_in_debug_output() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("servers.toml");
    fs::write(
        &path,
        "[[servers]]\ninstance_id = \"daemon-a\"\napi_key = \"never-log-me\"\n",
    )
    .expect("fixture writes");

    let credential = load_server_credentials(&path)
        .expect("fixture parses")
        .remove(0);
    let debug = format!("{credential:?}");
    assert!(debug.contains("[redacted]"));
    assert!(!debug.contains("never-log-me"));
}

#[test]
fn shared_server_parser_reports_the_source_path() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("servers.toml");
    fs::write(&path, "not = [valid").expect("fixture writes");

    let error = load_server_credentials(&path).expect_err("invalid TOML fails");
    assert!(matches!(error, StoredServersError::Parse { .. }));
    assert!(error.to_string().contains(path.to_string_lossy().as_ref()));
}
