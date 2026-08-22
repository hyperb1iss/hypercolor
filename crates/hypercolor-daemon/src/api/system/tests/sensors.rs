use super::*;

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
    let (_tx, rx) = watch::channel(snapshot);
    state
        .input_manager
        .lock()
        .await
        .set_sensor_snapshot_receiver(rx);

    let response = get_sensors(State(state)).await;
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("sensor body should read");
    let json: Value = serde_json::from_slice(&body).expect("sensor response should serialize");

    assert_eq!(json["data"]["cpu_load_percent"], 51.0);
    assert_eq!(json["data"]["cpu_temp_celsius"], 72.5);
    assert_eq!(json["data"]["polled_at_ms"], 1234);
}
