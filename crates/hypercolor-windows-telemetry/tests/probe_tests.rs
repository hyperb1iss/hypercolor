use hypercolor_windows_telemetry::{SensorExtras, SystemSnapshot, motherboard_info};

#[test]
fn sensor_extras_only_add_readings_on_windows() {
    let mut extras = SensorExtras::new();
    let mut snapshot = SystemSnapshot::empty();
    extras.merge_snapshot(&mut snapshot);

    if !cfg!(target_os = "windows") {
        assert!(snapshot.components.is_empty());
        assert_eq!(snapshot.cpu_temp_celsius, None);
    }
}

#[test]
fn motherboard_probe_is_absent_off_windows() {
    let info = motherboard_info();
    if !cfg!(target_os = "windows") {
        assert!(info.is_none());
    }
}
