//! The systemd watchdog keepalive consults the daemon's health on every
//! interval, starting with the first.

#[cfg(target_os = "linux")]
#[tokio::test]
async fn watchdog_consults_the_health_check_on_its_first_interval() {
    let (checked_tx, mut checked_rx) = tokio::sync::mpsc::unbounded_channel();

    hypercolor_linux_session::spawn_watchdog_while(move || {
        let _ = checked_tx.send(());
        false
    });

    tokio::time::timeout(std::time::Duration::from_secs(5), checked_rx.recv())
        .await
        .expect("the first keepalive interval should run the health check")
        .expect("the watchdog task should still hold the health check");
}
