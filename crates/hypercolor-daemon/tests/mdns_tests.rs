use std::net::SocketAddr;
use std::time::Duration;

use hypercolor_core::device::discover_servers;
use hypercolor_daemon::mdns::MdnsPublisher;
use hypercolor_types::server::ServerIdentity;
use uuid::Uuid;

fn unique_identity() -> ServerIdentity {
    let instance_id = Uuid::now_v7().to_string();
    let suffix: String = instance_id.chars().take(8).collect();
    ServerIdentity {
        instance_id,
        instance_name: format!("mdns-test-{suffix}"),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

#[tokio::test]
async fn mdns_publisher_skips_loopback_bind() {
    let publisher = MdnsPublisher::new(
        &unique_identity(),
        SocketAddr::from(([127, 0, 0, 1], 9420)),
        true,
        false,
    )
    .expect("loopback guard should not error");

    assert!(publisher.is_none());
}

#[tokio::test]
async fn mdns_round_trip_discovers_published_server() {
    let identity = unique_identity();
    let publisher = MdnsPublisher::new(
        &identity,
        SocketAddr::from(([0, 0, 0, 0], 59420)),
        true,
        true,
    )
    .expect("mDNS publisher should initialize")
    .expect("non-loopback bind should publish");

    tokio::time::sleep(Duration::from_millis(250)).await;

    let servers = discover_servers(Duration::from_secs(3))
        .await
        .expect("server discovery should succeed");
    let discovered = servers
        .into_iter()
        .find(|server| server.identity.instance_id == identity.instance_id)
        .expect("published server should be discoverable");

    assert_eq!(discovered.identity.instance_name, identity.instance_name);
    assert_eq!(discovered.identity.version, identity.version);
    assert_eq!(discovered.port, 59420);
    assert!(discovered.auth_required);

    publisher.shutdown().await;
}
