//! Host selection for local process and filesystem operations.

use hypercolor_cli::client::DaemonClient;

#[test]
fn local_host_operations_reject_remote_targets() {
    for host in ["localhost", "127.0.0.1", "127.0.0.2", "[::1]"] {
        assert!(DaemonClient::new(host, 9420, None).is_loopback(), "{host}");
    }
    for host in ["192.168.1.20", "localhost.example.com", "[2001:db8::1]"] {
        assert!(!DaemonClient::new(host, 9420, None).is_loopback(), "{host}");
    }
}

#[test]
fn local_identity_uses_advertised_directory_and_rejects_forwarded_instances() {
    use hypercolor_cli::client::verify_local_instance;
    let directory = tempfile::tempdir().expect("instance directory");
    assert!(verify_local_instance(directory.path(), "local-id").is_err());
    std::fs::write(directory.path().join("instance_id"), "local-id\n").expect("instance identity");
    assert!(verify_local_instance(directory.path(), "local-id").is_ok());
    assert!(verify_local_instance(directory.path(), "remote-id").is_err());
    assert!(verify_local_instance(directory.path(), "").is_err());
    assert!(verify_local_instance(std::path::Path::new("relative"), "local-id").is_err());
}
