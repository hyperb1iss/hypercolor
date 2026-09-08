use hypercolor_openrgb_host::verify_instance_directory;

#[test]
fn filesystem_identity_rejects_missing_or_forwarded_instances() {
    let directory = tempfile::tempdir().expect("directory");
    assert!(verify_instance_directory(directory.path(), "local").is_err());
    std::fs::write(directory.path().join("instance_id"), "local\n").expect("identity");
    assert!(verify_instance_directory(directory.path(), "local").is_ok());
    assert!(verify_instance_directory(directory.path(), "remote").is_err());
    assert!(verify_instance_directory(directory.path(), "").is_err());
    assert!(verify_instance_directory(std::path::Path::new("relative"), "local").is_err());
}
