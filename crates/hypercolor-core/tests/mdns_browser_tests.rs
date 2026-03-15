use std::time::Duration;

use hypercolor_core::device::net::MdnsBrowser;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use tokio::time::timeout;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test(flavor = "current_thread")]
async fn mdns_browser_discovers_registered_service_with_txt() -> TestResult {
    let publisher = ServiceDaemon::new()?;
    let txt = [("id", "demo"), ("kind", "test")];
    let service_info = ServiceInfo::new(
        "_wled._tcp.local.",
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

    let browser = MdnsBrowser::new()?;
    let services = browser
        .browse("_wled._tcp.local.", Duration::from_secs(2))
        .await?;
    let discovered = services
        .into_iter()
        .find(|service| service.port == 42424)
        .expect("browser should discover the published test service");

    assert!(!discovered.name.is_empty());
    assert!(discovered.host.is_ipv4());
    assert_eq!(discovered.txt.get("id"), Some(&"demo".to_owned()));
    assert_eq!(discovered.txt.get("kind"), Some(&"test".to_owned()));

    let unregister = publisher.unregister(&service_fullname)?;
    let _ = timeout(Duration::from_secs(1), unregister.recv_async()).await;
    let shutdown = publisher.shutdown()?;
    let _ = timeout(Duration::from_secs(1), shutdown.recv_async()).await;

    Ok(())
}
