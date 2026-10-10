use hypercolor_windows_telemetry::{
    SensorExtras, SystemSnapshot, motherboard_info, running_software,
};

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

#[test]
fn software_inventory_lists_this_process_on_windows_only() {
    let snapshot = running_software();
    if cfg!(target_os = "windows") {
        let snapshot = snapshot.expect("WMI should list processes on Windows");
        let me = std::env::current_exe().expect("current exe");
        let me = me
            .file_name()
            .and_then(|name| name.to_str())
            .expect("exe file name");
        assert!(
            snapshot
                .processes
                .iter()
                .any(|process| process.name.eq_ignore_ascii_case(me)),
            "the test binary {me} should be in the inventory"
        );
    } else {
        assert!(snapshot.is_none());
    }
}
