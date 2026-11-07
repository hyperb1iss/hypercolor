//! The shared session contract against the loopback fake.

mod support;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use hypercolor_driver_api::DeviceBackend;
use hypercolor_driver_conformance::{SessionRig, assert_session_contract, assert_udp_port_released};
use hypercolor_driver_wiz::config::WizConfig;
use hypercolor_driver_wiz::{
    RequestPolicy, WizBackend, WizScanOptions, WizScanner, discovered_device,
};
use hypercolor_types::device::DeviceId;
use support::{FakeSpec, FakeWiz, TestResult};

struct WizRig {
    fake: FakeWiz,
    backend: WizBackend,
    id: DeviceId,
    seen: usize,
}

#[async_trait]
impl SessionRig for WizRig {
    fn backend(&self) -> &dyn DeviceBackend {
        &self.backend
    }

    fn device_id(&self) -> DeviceId {
        self.id
    }

    fn frame(&self, shade: u8) -> Vec<[u8; 3]> {
        vec![[shade, 0, 255]]
    }

    async fn saw_traffic(&mut self, window: Duration) -> bool {
        let deadline = Instant::now() + window;
        while Instant::now() < deadline {
            let count = self.fake.pilots().len();
            if count > self.seen {
                self.seen = count;
                return true;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        false
    }

    async fn discard_traffic(&mut self) {
        self.seen = self.fake.pilots().len();
    }

    async fn assert_transport_released(&mut self) {
        let source = self.fake.last_pilot_source().expect("a pilot arrived");
        assert_udp_port_released(source.port()).await;
    }
}

#[tokio::test]
async fn wiz_sessions_keep_the_contract() -> TestResult {
    let fake = FakeWiz::spawn(FakeSpec::color_bulb()).await?;
    let policy = RequestPolicy {
        reply_timeout: Duration::from_millis(200),
        attempts: 2,
    };
    let profiles = WizScanner::new(WizScanOptions {
        unicast_targets: vec![fake.addr()],
        broadcast_targets: Vec::new(),
        tracked: HashMap::new(),
        timeout: Duration::from_millis(800),
        request_policy: policy,
    })
    .scan()
    .await?
    .found;
    let discovered = discovered_device(profiles.first().ok_or("no device")?, 20);
    let backend = WizBackend::new(WizConfig::default()).with_request_policy(policy);
    backend.adopt_device(&discovered)?;

    let mut rig = WizRig {
        fake,
        backend,
        id: discovered.info.id,
        seen: 0,
    };
    assert_session_contract(&mut rig).await;
    Ok(())
}
