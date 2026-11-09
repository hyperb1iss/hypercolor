//! The shared session contract against the loopback fake.

mod support;

use std::time::{Duration, Instant};

use async_trait::async_trait;
use hypercolor_driver_api::DeviceBackend;
use hypercolor_driver_conformance::{
    SessionRig, assert_session_contract, assert_udp_port_released,
};
use hypercolor_driver_lifx::LifxBackend;
use hypercolor_driver_lifx::config::LifxConfig;
use hypercolor_driver_lifx::scanner::{LifxScanOptions, LifxScanner, discovered_device};
use hypercolor_driver_lifx::transport::RequestPolicy;
use hypercolor_types::device::DeviceId;
use support::{FakeDevice, FakeKind, FakeLifx, TestResult};

const ZONES: usize = 82;
const SET_EXTENDED_COLOR_ZONES: u16 = 510;

struct LifxRig {
    fake: FakeLifx,
    backend: LifxBackend,
    id: DeviceId,
    seen: usize,
}

impl LifxRig {
    fn frames(&self) -> usize {
        self.fake.received_of_type(SET_EXTENDED_COLOR_ZONES).len()
    }
}

#[async_trait]
impl SessionRig for LifxRig {
    fn backend(&self) -> &dyn DeviceBackend {
        &self.backend
    }

    fn device_id(&self) -> DeviceId {
        self.id
    }

    fn frame(&self, shade: u8) -> Vec<[u8; 3]> {
        vec![[shade, 0, 255]; ZONES]
    }

    async fn saw_traffic(&mut self, window: Duration) -> bool {
        let deadline = Instant::now() + window;
        while Instant::now() < deadline {
            let count = self.frames();
            if count > self.seen {
                self.seen = count;
                return true;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        false
    }

    async fn discard_traffic(&mut self) {
        self.seen = self.frames();
    }

    async fn assert_transport_released(&mut self) {
        let frames = self.fake.received_of_type(SET_EXTENDED_COLOR_ZONES);
        let from = frames.last().expect("a frame arrived").from;
        assert_udp_port_released(from.port()).await;
    }
}

#[tokio::test]
async fn lifx_sessions_keep_the_contract() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(
        3,
        38,
        (3, 70),
        FakeKind::ExtendedStrip { zones: 82 },
    )])
    .await?;
    let policy = RequestPolicy {
        reply_timeout: Duration::from_millis(200),
        attempts: 2,
    };
    let config = LifxConfig::default();
    let profiles = LifxScanner::new(LifxScanOptions {
        unicast_targets: vec![fake.addr()],
        broadcast_targets: Vec::new(),
        timeout: Duration::from_millis(600),
        request_policy: policy,
        ..LifxScanOptions::default()
    })
    .scan()
    .await?
    .found;
    let profile = profiles.first().ok_or("no device")?;
    let discovered = discovered_device(profile, config.max_messages_per_second);
    let backend = LifxBackend::new(config).with_request_policy(policy);
    backend.adopt_device(&discovered)?;

    let mut rig = LifxRig {
        fake,
        backend,
        id: discovered.info.id,
        seen: 0,
    };
    assert_session_contract(&mut rig).await;
    Ok(())
}
