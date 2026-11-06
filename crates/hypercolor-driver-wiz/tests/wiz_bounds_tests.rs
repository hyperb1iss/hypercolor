//! Bounds on untrusted LAN input: registration replies, probe fanout, and
//! the signing key's debug output (Spec 93 §3.1 and §5).

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hypercolor_driver_wiz::config::WizConfig;
use hypercolor_driver_wiz::scanner::{
    MAX_CONCURRENT_PROBES, collect_registrations, registration_mac,
};
use hypercolor_driver_wiz::{RequestPolicy, WizScanOptions, WizScanner};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// A responder on `ip` that answers every datagram with `reply`.
async fn responder(ip: Ipv4Addr, reply: String) -> TestResult<(SocketAddr, JoinHandle<()>)> {
    let socket = UdpSocket::bind((ip, 0)).await?;
    let addr = socket.local_addr()?;
    let task = tokio::spawn(async move {
        let mut buffer = [0_u8; 512];
        while let Ok((_, source)) = socket.recv_from(&mut buffer).await {
            let _ = socket.send_to(reply.as_bytes(), source).await;
        }
    });
    Ok((addr, task))
}

fn registration(mac: &str, success: bool) -> String {
    serde_json::json!({
        "method": "registration",
        "env": "pro",
        "result": { "mac": mac, "success": success },
    })
    .to_string()
}

#[test]
fn registration_replies_need_success_and_a_real_mac() {
    assert_eq!(
        registration_mac(registration("A8BB50000001", true).as_bytes()).as_deref(),
        Ok("a8:bb:50:00:00:01")
    );
    assert!(registration_mac(registration("x", true).as_bytes()).is_err());
    assert!(registration_mac(registration("a8bb50000001", false).as_bytes()).is_err());
    assert!(registration_mac(br#"{"method":"registration","result":{"mac":"x"}}"#).is_err());
    assert!(registration_mac(br#"{"method":"getPilot","result":{"mac":"a8bb50000001"}}"#).is_err());
}

#[tokio::test]
async fn invented_macs_from_many_ports_admit_nothing() -> TestResult {
    let mut targets = Vec::new();
    let mut tasks = Vec::new();
    for _ in 0..12 {
        let (addr, task) = responder(
            Ipv4Addr::LOCALHOST,
            r#"{"method":"registration","result":{"mac":"x"}}"#.to_owned(),
        )
        .await?;
        targets.push(addr);
        tasks.push(task);
    }
    let found = collect_registrations(&targets, Duration::from_millis(300), 64).await?;
    tasks.iter().for_each(JoinHandle::abort);
    assert!(
        found.is_empty(),
        "malformed replies are never admitted: {found:?}"
    );
    Ok(())
}

#[tokio::test]
async fn one_source_address_is_admitted_once() -> TestResult {
    let mut targets = Vec::new();
    let mut tasks = Vec::new();
    for index in 0..12_u8 {
        let (addr, task) = responder(
            Ipv4Addr::LOCALHOST,
            registration(&format!("a8bb500000{index:02x}"), true),
        )
        .await?;
        targets.push(addr);
        tasks.push(task);
    }
    let found = collect_registrations(&targets, Duration::from_millis(300), 64).await?;
    tasks.iter().for_each(JoinHandle::abort);
    assert_eq!(
        found.len(),
        1,
        "distinct MACs from one host count once: {found:?}"
    );
    Ok(())
}

/// Linux routes all of 127.0.0.0/8 to the loopback interface, so distinct
/// source addresses can be tested there without leaving the host.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn admission_stops_at_its_limit() -> TestResult {
    let mut targets = Vec::new();
    let mut tasks = Vec::new();
    for host in 2..=5_u8 {
        let (addr, task) = responder(
            Ipv4Addr::new(127, 0, 0, host),
            registration(&format!("a8bb500000{host:02x}"), true),
        )
        .await?;
        targets.push(addr);
        tasks.push(task);
    }
    let found = collect_registrations(&targets, Duration::from_millis(400), 2).await?;
    tasks.iter().for_each(JoinHandle::abort);
    assert_eq!(found.len(), 2, "admission stops at the limit: {found:?}");
    Ok(())
}

#[tokio::test]
async fn probes_run_with_bounded_concurrency() -> TestResult {
    // Silent devices that record when their first request arrived.
    let first_seen: Arc<Mutex<Vec<Instant>>> = Arc::new(Mutex::new(Vec::new()));
    let mut targets = Vec::new();
    let mut tasks = Vec::new();
    for _ in 0..(MAX_CONCURRENT_PROBES * 3) {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        targets.push(socket.local_addr()?);
        let first_seen = Arc::clone(&first_seen);
        tasks.push(tokio::spawn(async move {
            let mut buffer = [0_u8; 512];
            if socket.recv_from(&mut buffer).await.is_ok() {
                first_seen.lock().expect("lock").push(Instant::now());
            }
        }));
    }
    let started = Instant::now();
    let profiles = WizScanner::new(WizScanOptions {
        unicast_targets: targets,
        broadcast_targets: Vec::new(),
        timeout: Duration::from_millis(2_000),
        request_policy: RequestPolicy {
            reply_timeout: Duration::from_millis(200),
            attempts: 1,
        },
    })
    .scan_profiles()
    .await?;
    tasks.iter().for_each(JoinHandle::abort);
    assert!(profiles.is_empty());
    let early = first_seen
        .lock()
        .expect("lock")
        .iter()
        .filter(|seen| seen.duration_since(started) < Duration::from_millis(100))
        .count();
    assert!(
        early <= MAX_CONCURRENT_PROBES,
        "at most {MAX_CONCURRENT_PROBES} probes start at once, saw {early}"
    );
    assert!(started.elapsed() < Duration::from_secs(4));
    Ok(())
}

#[test]
fn debug_output_redacts_the_signing_key() {
    let config = WizConfig {
        signing_key: Some("00112233445566778899aabbccddeeff".to_owned()),
        ..WizConfig::default()
    };
    let debug = format!("{config:?}");
    assert!(
        !debug.contains("00112233445566778899aabbccddeeff"),
        "{debug}"
    );
    assert!(debug.contains("<redacted>"), "{debug}");
    assert!(format!("{:?}", WizConfig::default()).contains("signing_key: None"));
}

#[tokio::test]
async fn extreme_retry_policies_do_not_overflow() -> TestResult {
    let profiles = WizScanner::new(WizScanOptions {
        unicast_targets: Vec::new(),
        broadcast_targets: Vec::new(),
        timeout: Duration::from_millis(100),
        request_policy: RequestPolicy {
            reply_timeout: Duration::MAX,
            attempts: u32::MAX,
        },
    })
    .scan_profiles()
    .await?;
    assert!(profiles.is_empty());
    Ok(())
}
