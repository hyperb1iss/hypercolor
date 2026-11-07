//! The shared session contract against the loopback fake.

mod support;

use std::time::{Duration, Instant};

use async_trait::async_trait;
use hypercolor_driver_api::DeviceBackend;
use hypercolor_driver_conformance::{
    SessionRig, assert_session_contract, assert_udp_port_released,
};
use hypercolor_driver_twinkly::config::TwinklyConfig;
use hypercolor_driver_twinkly::{
    TwinklyBackend, TwinklyScanOptions, TwinklyScanner, discovered_device,
};
use hypercolor_types::device::DeviceId;
use support::{FakeSpec, FakeTwinkly, TestResult};

const LEDS: usize = 4;

struct TwinklyRig {
    fake: FakeTwinkly,
    backend: TwinklyBackend,
    id: DeviceId,
    seen: usize,
}

#[async_trait]
impl SessionRig for TwinklyRig {
    fn backend(&self) -> &dyn DeviceBackend {
        &self.backend
    }

    fn device_id(&self) -> DeviceId {
        self.id
    }

    fn frame(&self, shade: u8) -> Vec<[u8; 3]> {
        vec![[shade, 0, 255]; LEDS]
    }

    async fn saw_traffic(&mut self, window: Duration) -> bool {
        let deadline = Instant::now() + window;
        while Instant::now() < deadline {
            let count = self.fake.datagrams().len();
            if count > self.seen {
                self.seen = count;
                return true;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        false
    }

    async fn discard_traffic(&mut self) {
        self.seen = self.fake.datagrams().len();
    }

    async fn assert_transport_released(&mut self) {
        let source = self.fake.last_rt_source().expect("a datagram arrived");
        assert_udp_port_released(source.port()).await;
    }
}

#[tokio::test]
async fn twinkly_sessions_keep_the_contract() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec::gen2_rgb(4)).await?;
    let profiles = TwinklyScanner::new(TwinklyScanOptions::new(
        vec![fake.endpoint()],
        Vec::new(),
        Duration::from_millis(800),
    ))
    .scan()
    .await?
    .found;
    let discovered = discovered_device(profiles.first().ok_or("no device")?, 60);
    let backend = TwinklyBackend::new(TwinklyConfig::default());
    backend.adopt_device(&discovered)?;

    let mut rig = TwinklyRig {
        fake,
        backend,
        id: discovered.info.id,
        seen: 0,
    };
    assert_session_contract(&mut rig).await;
    Ok(())
}

#[tokio::test]
async fn disconnect_restores_the_mode_after_upkeep_has_stopped() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec::gen2_rgb(4)).await?;
    fake.set_mode("movie");
    let profiles = TwinklyScanner::new(TwinklyScanOptions::new(
        vec![fake.endpoint()],
        Vec::new(),
        Duration::from_millis(800),
    ))
    .scan()
    .await?
    .found;
    let discovered = discovered_device(profiles.first().ok_or("no device")?, 60);
    let backend = TwinklyBackend::new(TwinklyConfig::default())
        .with_upkeep_interval(Duration::from_millis(5));
    backend.adopt_device(&discovered)?;
    let id = discovered.info.id;

    backend.connect(&id).await?;
    tokio::time::sleep(Duration::from_millis(40)).await;
    backend.disconnect(&id).await?;

    // Upkeep is joined before the restore, so it cannot flip the device back.
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(fake.mode(), "movie");
    Ok(())
}
