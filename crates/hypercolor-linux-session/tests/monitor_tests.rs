use hypercolor_types::session::SessionConfig;

#[test]
fn platform_monitor_set_matches_host_support() {
    let monitors = hypercolor_linux_session::monitors(&SessionConfig::default());
    let names: Vec<_> = monitors.iter().map(|monitor| monitor.name()).collect();

    #[cfg(target_os = "linux")]
    assert_eq!(names, ["screensaver", "logind"]);

    #[cfg(not(target_os = "linux"))]
    assert!(names.is_empty());
}
