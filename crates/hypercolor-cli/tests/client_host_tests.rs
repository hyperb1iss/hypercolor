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
