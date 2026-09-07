const TRANSPORT_SOURCES: &[&str] = &[
    include_str!("../src/transport/hidapi.rs"),
    include_str!("../src/transport/hidraw.rs"),
    include_str!("../src/transport/serial.rs"),
    include_str!("../src/transport/smbus.rs"),
];

#[test]
fn transport_error_classifiers_do_not_normalize_display_text() {
    for source in TRANSPORT_SOURCES {
        assert!(!source.contains("detail.to_ascii_lowercase()"));
        assert!(!source.contains("let lowered = detail.to_ascii_lowercase()"));
    }
}
