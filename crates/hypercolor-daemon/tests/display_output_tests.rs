use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use async_trait::async_trait;
use tokio::sync::Mutex;

use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_core::device::{BackendInfo, BackendManager, DeviceBackend, DeviceRegistry};
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceId, DeviceInfo,
    DeviceState, DeviceTopologyHint, ZoneInfo,
};

use hypercolor_daemon::display_output::{DisplayOutputState, DisplayOutputThread};

struct RecordingDisplayBackend {
    expected_device_id: DeviceId,
    connected: bool,
    display_writes: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl RecordingDisplayBackend {
    fn new(expected_device_id: DeviceId, display_writes: Arc<Mutex<Vec<Vec<u8>>>>) -> Self {
        Self {
            expected_device_id,
            connected: false,
            display_writes,
        }
    }
}

#[async_trait]
impl DeviceBackend for RecordingDisplayBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "usb".to_owned(),
            name: "USB Recording".to_owned(),
            description: "Test backend for display output".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        Ok(Vec::new())
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }

        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }

        self.connected = false;
        Ok(())
    }

    async fn write_colors(&mut self, id: &DeviceId, _colors: &[[u8; 3]]) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }

        Ok(())
    }

    async fn write_display_frame(&mut self, id: &DeviceId, jpeg_data: &[u8]) -> Result<()> {
        if *id != self.expected_device_id {
            bail!("unexpected device id {id}");
        }
        if !self.connected {
            bail!("display write while disconnected");
        }

        self.display_writes.lock().await.push(jpeg_data.to_vec());
        Ok(())
    }
}

fn display_device_info(device_id: DeviceId, has_display: bool) -> DeviceInfo {
    let zones = if has_display {
        vec![ZoneInfo {
            name: "Display".to_owned(),
            led_count: 0,
            topology: DeviceTopologyHint::Display {
                width: 480,
                height: 480,
                circular: true,
            },
            color_format: DeviceColorFormat::Jpeg,
        }]
    } else {
        vec![ZoneInfo {
            name: "Ring".to_owned(),
            led_count: 24,
            topology: DeviceTopologyHint::Ring { count: 24 },
            color_format: DeviceColorFormat::Rgb,
        }]
    };

    DeviceInfo {
        id: device_id,
        name: "Corsair Test Device".to_owned(),
        vendor: "Corsair".to_owned(),
        family: DeviceFamily::Corsair,
        model: None,
        connection_type: ConnectionType::Usb,
        zones,
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: if has_display { 0 } else { 24 },
            supports_direct: !has_display,
            supports_brightness: false,
            has_display,
            display_resolution: has_display.then_some((480, 480)),
            max_fps: 30,
        },
    }
}

fn sample_canvas() -> Canvas {
    let mut canvas = Canvas::new(320, 200);
    canvas.fill(Rgba::new(8, 16, 24, 255));
    canvas.set_pixel(0, 0, Rgba::new(255, 0, 0, 255));
    canvas.set_pixel(319, 0, Rgba::new(0, 255, 0, 255));
    canvas.set_pixel(0, 199, Rgba::new(0, 0, 255, 255));
    canvas.set_pixel(319, 199, Rgba::new(255, 255, 255, 255));
    canvas
}

async fn wait_for_display_writes(display_writes: &Arc<Mutex<Vec<Vec<u8>>>>) -> Vec<Vec<u8>> {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let writes = display_writes.lock().await.clone();
            if !writes.is_empty() {
                return writes;
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("display output should arrive within timeout")
}

#[tokio::test]
async fn automatic_display_output_mirrors_canvas_to_connected_display_devices() {
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(RecordingDisplayBackend::new(
        device_id,
        Arc::clone(&display_writes),
    )));
    backend_manager
        .connect_device("usb", device_id, "corsair:test-display")
        .await
        .expect("backend should connect");

    let tracked_id = device_registry
        .add(display_device_info(device_id, true))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        event_bus: Arc::clone(&event_bus),
    });

    let canvas = sample_canvas();
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&canvas, 1, 16));

    let writes = wait_for_display_writes(&display_writes).await;
    assert!(!writes[0].is_empty());
    assert_eq!(&writes[0][..3], &[0xFF, 0xD8, 0xFF]);

    thread.shutdown().await.expect("display thread should stop");
}

#[tokio::test]
async fn automatic_display_output_skips_devices_without_display_capabilities() {
    let event_bus = Arc::new(HypercolorBus::new());
    let device_registry = DeviceRegistry::new();
    let display_writes = Arc::new(Mutex::new(Vec::new()));
    let device_id = DeviceId::new();

    let mut backend_manager = BackendManager::new();
    backend_manager.register_backend(Box::new(RecordingDisplayBackend::new(
        device_id,
        Arc::clone(&display_writes),
    )));

    let tracked_id = device_registry
        .add(display_device_info(device_id, false))
        .await;
    assert_eq!(tracked_id, device_id);
    assert!(
        device_registry
            .set_state(&device_id, DeviceState::Active)
            .await
    );

    let mut thread = DisplayOutputThread::spawn(DisplayOutputState {
        backend_manager: Arc::new(Mutex::new(backend_manager)),
        device_registry: device_registry.clone(),
        event_bus: Arc::clone(&event_bus),
    });

    let canvas = sample_canvas();
    let _ = event_bus
        .canvas_sender()
        .send(CanvasFrame::from_canvas(&canvas, 1, 16));
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(display_writes.lock().await.is_empty());

    thread.shutdown().await.expect("display thread should stop");
}
