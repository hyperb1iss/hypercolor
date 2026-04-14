#![allow(clippy::float_cmp)]

use hypercolor_types::sensor::{SensorReading, SensorUnit, SystemSnapshot};

#[test]
fn system_snapshot_empty_is_valid() {
    let snapshot = SystemSnapshot::empty();

    assert_eq!(snapshot.cpu_load_percent, 0.0);
    assert!(snapshot.cpu_loads.is_empty());
    assert!(snapshot.cpu_temp_celsius.is_none());
    assert!(snapshot.gpu_temp_celsius.is_none());
    assert_eq!(snapshot.ram_used_percent, 0.0);
    assert_eq!(snapshot.polled_at_ms, 0);
}

#[test]
fn system_snapshot_round_trips_through_json() {
    let snapshot = SystemSnapshot {
        cpu_load_percent: 42.5,
        cpu_loads: vec![40.0, 45.0],
        cpu_temp_celsius: Some(71.25),
        gpu_temp_celsius: Some(63.5),
        gpu_load_percent: Some(88.0),
        gpu_vram_used_mb: Some(2048.0),
        ram_used_percent: 55.5,
        ram_used_mb: 8192.0,
        ram_total_mb: 16384.0,
        components: vec![SensorReading::new(
            "nvme_temp",
            52.0,
            SensorUnit::Celsius,
            None,
            Some(90.0),
            Some(95.0),
        )],
        polled_at_ms: 1_746_912_345,
    };

    let json = serde_json::to_string(&snapshot).expect("snapshot should serialize");
    let decoded: SystemSnapshot = serde_json::from_str(&json).expect("snapshot should deserialize");

    assert_eq!(decoded, snapshot);
}

#[test]
fn system_snapshot_finds_well_known_and_normalized_labels() {
    let snapshot = SystemSnapshot {
        cpu_load_percent: 65.0,
        cpu_loads: vec![65.0],
        cpu_temp_celsius: Some(74.0),
        gpu_temp_celsius: None,
        gpu_load_percent: None,
        gpu_vram_used_mb: None,
        ram_used_percent: 50.0,
        ram_used_mb: 4096.0,
        ram_total_mb: 8192.0,
        components: vec![SensorReading::new(
            "Package id 0",
            74.0,
            SensorUnit::Celsius,
            None,
            Some(100.0),
            None,
        )],
        polled_at_ms: 99,
    };

    let cpu_temp = snapshot
        .reading("cpu_temp")
        .expect("well-known reading should resolve");
    assert_eq!(cpu_temp.value, 74.0);
    assert_eq!(cpu_temp.unit.symbol(), "°C");

    let raw = snapshot
        .reading("package-id-0")
        .expect("normalized raw component label should resolve");
    assert_eq!(raw.value, 74.0);
}
