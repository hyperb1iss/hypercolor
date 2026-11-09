use super::*;

#[tokio::test]
async fn rediscovery_notifies_metadata_changes_without_a_lifecycle_transition() {
    let temp = tempfile::tempdir().expect("temp directory");
    let driver = LaneDiscoveryDriver::new(&LANE_QUICK_DRIVER);
    let runtime = lane_runtime(&temp, &[&driver]);
    let mut device = lane_device(&LANE_QUICK_DRIVER, "panels");
    let panel = device.info.segments[0].clone();
    device.info.segments.clear();
    device.info.capabilities.led_count = 0;
    let id = runtime.device_registry.add_discovered(device.clone()).await;
    let actions = runtime
        .lifecycle_manager
        .lock()
        .await
        .on_discovered_with_behavior(
            id,
            &device.info,
            Some(&device.fingerprint),
            DiscoveryConnectBehavior::Deferred,
        );
    assert!(actions.is_empty());
    device.info.segments = (0..21)
        .map(|index| SegmentInfo {
            name: format!("Panel {index}"),
            led_count: 1,
            ..panel.clone()
        })
        .collect();
    device.info.capabilities.led_count = 21;
    driver.report(vec![device]);
    let mut events = runtime.event_bus.subscribe_all();

    for expected_notifications in [1, 0] {
        run_complete_sweep(
            &runtime,
            vec![DiscoveryTarget::driver(LANE_QUICK_DRIVER.id)],
        )
        .await;
        let tracked = runtime
            .device_registry
            .get(&id)
            .await
            .expect("tracked panels");
        assert_eq!(tracked.info.segments.len(), 21);
        assert_eq!(tracked.state, DeviceState::Known);
        let mut notifications = Vec::new();
        while let Ok(event) = events.try_recv() {
            if let HypercolorEvent::DeviceStateChanged { device_id, changes } = event.event
                && device_id == id.to_string()
                && let Some(info) = changes.get("info")
            {
                notifications.push(info.clone());
            }
        }
        assert_eq!(notifications.len(), expected_notifications);
        if let Some(info) = notifications.first() {
            let info: DeviceInfo =
                serde_json::from_value(info.clone()).expect("device metadata event");
            assert_eq!(info, tracked.info);
        }
    }
}
