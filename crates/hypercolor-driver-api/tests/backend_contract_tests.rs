use std::sync::Arc;

use async_trait::async_trait;
use hypercolor_driver_api::DeviceFrameSink;
use hypercolor_types::device::DeviceError;

struct LegacyFrameSink;

#[async_trait]
impl DeviceFrameSink for LegacyFrameSink {
    async fn write_colors_shared(&self, _colors: Arc<Vec<[u8; 3]>>) -> Result<(), DeviceError> {
        Ok(())
    }
}

#[test]
fn a_legacy_frame_sink_defaults_to_open_without_a_liveness_override() {
    let sink: Arc<dyn DeviceFrameSink> = Arc::new(LegacyFrameSink);

    assert!(!sink.is_closed());
}
