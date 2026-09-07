use std::path::Path;

fn daemon_source(path: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|error| panic!("failed to read {path}: {error}"))
}

#[test]
fn rest_and_websocket_metrics_share_one_render_health_collector() {
    let performance = daemon_source("src/performance.rs");
    assert!(performance.contains("pub(crate) fn render_health_counts()"));

    for transport in ["src/api/system/metrics.rs", "src/api/ws/relays.rs"] {
        let source = daemon_source(transport);
        assert!(
            source.contains("render_health_counts()"),
            "{transport} bypasses the shared render health collector"
        );
        for retired_collector in [
            "fn servo_effect_health_counts()",
            "fn render_pipeline_health_counts()",
            "fn gpu_sparkleflinger_health_counts()",
        ] {
            assert!(
                !source.contains(retired_collector),
                "{transport} redeclares {retired_collector}"
            );
        }
    }
}
