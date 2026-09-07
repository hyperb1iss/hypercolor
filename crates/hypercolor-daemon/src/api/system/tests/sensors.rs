use super::*;
use hypercolor_core::input::{DataSourceKind, InputData};

struct FixedSensorSource {
    snapshot: Arc<SystemSnapshot>,
    running: bool,
}

impl hypercolor_core::input::InputSource for FixedSensorSource {
    fn name(&self) -> &'static str {
        "fixed-sensors"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::Sensors(Arc::clone(&self.snapshot)))
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

impl hypercolor_core::input::SourceRoleBinding for FixedSensorSource {
    type Role = hypercolor_core::input::DataSourceRole;
}

impl hypercolor_core::input::DataSource for FixedSensorSource {
    fn data_source_kind(&self) -> DataSourceKind {
        DataSourceKind::Sensors
    }
}

#[tokio::test]
async fn sensors_endpoint_returns_latest_snapshot() {
    let state = Arc::new(AppState::new());
    let snapshot = Arc::new(SystemSnapshot {
        cpu_load_percent: 51.0,
        cpu_loads: vec![48.0, 54.0],
        cpu_temp_celsius: Some(72.5),
        gpu_temp_celsius: None,
        gpu_load_percent: None,
        gpu_vram_used_mb: None,
        ram_used_percent: 44.0,
        ram_used_mb: 8192.0,
        ram_total_mb: 16384.0,
        components: vec![SensorReading::new(
            "Package id 0",
            72.5,
            SensorUnit::Celsius,
            None,
            Some(100.0),
            None,
        )],
        polled_at_ms: 1234,
    });
    state
        .input_manager()
        .add_source(hypercolor_core::input::ManagedSourceRole::data(Box::new(
            FixedSensorSource {
                snapshot,
                running: false,
            },
        )))
        .expect("sensor source should register");
    state
        .input_manager()
        .start_all()
        .expect("sensor source should start");
    state.input_manager().sample_sources(0.0);

    let response = get_sensors(State(state)).await;
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("sensor body should read");
    let json: Value = serde_json::from_slice(&body).expect("sensor response should serialize");

    assert_eq!(json["data"]["cpu_load_percent"], 51.0);
    assert_eq!(json["data"]["cpu_temp_celsius"], 72.5);
    assert_eq!(json["data"]["polled_at_ms"], 1234);
}
