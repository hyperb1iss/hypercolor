use std::time::Duration;

use hypercolor_driver_support::{MdnsBrowser, MdnsScope};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use tokio::time::timeout;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

// Publisher and browser both stay on loopback so the test never sends
// mDNS onto the developer's LAN.
#[tokio::test(flavor = "current_thread")]
async fn mdns_browser_discovers_registered_service_with_txt() -> TestResult {
    let publisher = ServiceDaemon::new()?;
    MdnsScope::Loopback.restrict(&publisher)?;
    let txt = [("id", "demo"), ("kind", "test")];
    let service_info = ServiceInfo::new(
        "_hypercolor-test._tcp.local.",
        "mdns-browser-test",
        "mdns-browser.local.",
        "",
        42424,
        &txt[..],
    )?
    .enable_addr_auto();
    let service_fullname = service_info.get_fullname().to_owned();

    publisher.register(service_info)?;
    tokio::time::sleep(Duration::from_millis(250)).await;

    let browser = MdnsBrowser::with_scope(MdnsScope::Loopback)?;
    let services = browser
        .browse("_hypercolor-test._tcp.local.", Duration::from_secs(2))
        .await?;
    let discovered = services
        .into_iter()
        .find(|service| service.port == 42424)
        .expect("browser should discover the published test service");

    assert!(!discovered.name.is_empty());
    assert!(discovered.host.is_ipv4());
    assert!(
        discovered.host.is_loopback(),
        "a loopback-scoped browse should resolve only loopback addresses, got {}",
        discovered.host
    );
    assert_eq!(discovered.txt.get("id"), Some(&"demo".to_owned()));
    assert_eq!(discovered.txt.get("kind"), Some(&"test".to_owned()));

    let unregister = publisher.unregister(&service_fullname)?;
    let _ = timeout(Duration::from_secs(1), unregister.recv_async()).await;
    let shutdown = publisher.shutdown()?;
    let _ = timeout(Duration::from_secs(1), shutdown.recv_async()).await;

    Ok(())
}

#[test]
fn mdns_scope_defaults_to_the_network() {
    assert_eq!(MdnsScope::default(), MdnsScope::Network);
}
