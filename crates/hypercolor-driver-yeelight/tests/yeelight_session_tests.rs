//! The shared session contract against the loopback fake.

mod support;

use std::time::{Duration, Instant};

use async_trait::async_trait;
use hypercolor_driver_api::DeviceBackend;
use hypercolor_driver_conformance::{SessionRig, assert_session_contract};
use hypercolor_driver_yeelight::config::YeelightConfig;
use hypercolor_driver_yeelight::{
    YeelightBackend, YeelightScanOptions, YeelightScanner, discovered_device,
};
use hypercolor_types::device::DeviceId;
use support::{FakeSpec, FakeYeelight, TestResult};

struct YeelightRig {
    fake: FakeYeelight,
    backend: YeelightBackend,
    id: DeviceId,
    seen: usize,
    closes: usize,
}

impl YeelightRig {
    fn music_count(&self) -> usize {
        self.fake.music_lines().len()
    }
}

#[async_trait]
impl SessionRig for YeelightRig {
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
            let count = self.music_count();
            if count > self.seen {
                self.seen = count;
                return true;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        false
    }

    async fn discard_traffic(&mut self) {
        self.seen = self.music_count();
    }

    async fn assert_transport_released(&mut self) {
        // Each session holds one control and one music connection; the
        // peer sees both reach EOF.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let (control, music) = self.fake.closed_connections();
            if control > self.closes && music > self.closes {
                self.closes += 1;
                return;
            }
            assert!(
                Instant::now() < deadline,
                "a closed session left a connection open ({control} control, {music} music)"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

#[tokio::test]
async fn yeelight_sessions_keep_the_contract() -> TestResult {
    let fake = FakeYeelight::spawn(FakeSpec::color_bulb()).await?;
    let config = YeelightConfig::default();
    let profiles = YeelightScanner::new(YeelightScanOptions {
        unicast_targets: vec![fake.search_addr()],
        multicast: false,
        timeout: Duration::from_millis(1_000),
    })
    .scan()
    .await?
    .found;
    let profile = profiles.first().ok_or("no device")?;
    let discovered = discovered_device(profile, config.max_fps, config.music_mode);
    let backend = YeelightBackend::new(config);
    backend.adopt_device(&discovered)?;

    let mut rig = YeelightRig {
        fake,
        backend,
        id: discovered.info.id,
        seen: 0,
        closes: 0,
    };
    assert_session_contract(&mut rig).await;
    Ok(())
}
