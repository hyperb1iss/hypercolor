//! Source fences for the daemon's internal API ownership boundaries.

use std::path::{Path, PathBuf};

fn daemon_sources() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut pending = vec![root.clone()];
    let mut sources = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).expect("daemon source directory should read") {
            let path = entry.expect("daemon source entry should read").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let source = std::fs::read_to_string(&path)
                    .expect("daemon Rust source should read as UTF-8");
                sources.push((path, source));
            }
        }
    }
    sources
}

#[test]
fn app_state_has_one_module_identity() {
    let banned = [
        "crate::api::AppState",
        "hypercolor_daemon::api::AppState",
        "pub use crate::app_state::AppState",
    ];
    let offenders = daemon_sources()
        .into_iter()
        .flat_map(|(path, source)| {
            banned
                .into_iter()
                .filter(|pattern| source.contains(pattern))
                .map(|pattern| format!("{} contains {pattern}", path.display()))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "AppState belongs to app_state, not the transport API:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn driver_host_and_workers_reuse_the_discovery_context() {
    let sources = daemon_sources();
    let source = |suffix: &str| {
        sources
            .iter()
            .find(|(path, _)| path.ends_with(suffix))
            .map(|(_, source)| source.as_str())
            .unwrap_or_else(|| panic!("missing daemon source {suffix}"))
    };
    let host = source("network/host.rs");
    assert!(host.contains("runtime: DiscoveryRuntime"));
    assert!(!host.contains("clippy::too_many_arguments"));
    assert!(!host.contains("DiscoveryRuntime {\n            device_registry:"));

    let worker = source("startup/discovery_worker.rs");
    assert!(worker.contains("discovery: DiscoveryRuntime"));
    assert!(!worker.contains("DiscoveryRuntime {\n            device_registry:"));
}

#[test]
fn application_state_reuses_one_domain_graph() {
    let sources = daemon_sources();
    let source = |suffix: &str| {
        sources
            .iter()
            .find(|(path, _)| path.ends_with(suffix))
            .map(|(_, source)| source.as_str())
            .unwrap_or_else(|| panic!("missing daemon source {suffix}"))
    };
    let app_state = source("app_state.rs");
    assert!(app_state.contains("pub domains: DomainContexts"));
    assert!(app_state.contains("let domains = daemon.domains.clone();"));
    assert!(!app_state.contains("pub effect_registry:"));
    for retired_field in [
        "pub scene: SceneContext",
        "pub runtime_session: RuntimeSessionService",
        "pub devices: DeviceContext",
        "pub layout: LayoutContext",
        "pub output: OutputContext",
        "pub effects: EffectContext",
        "pub scene_tree: SceneTreeContext",
        "pub scene_library: SceneLibraryContext",
    ] {
        assert!(!app_state.contains(retired_field), "found {retired_field}");
    }

    let startup = source("startup/mod.rs");
    assert!(startup.contains("pub domains: DomainContexts"));
}

#[test]
fn transports_use_the_effect_domain_authority() {
    let offenders = daemon_sources()
        .into_iter()
        .filter(|(path, _)| {
            path.components()
                .any(|component| component.as_os_str() == "api" || component.as_os_str() == "mcp")
        })
        .filter(|(_, source)| source.contains("state.effect_registry"))
        .map(|(path, _)| path.display().to_string())
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "transport adapters bypassed EffectContext:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn layout_transport_uses_the_layout_domain_authority() {
    let sources = daemon_sources();
    let source = |suffix: &str| {
        sources
            .iter()
            .find(|(path, _)| path.ends_with(suffix))
            .map(|(_, source)| source.as_str())
            .unwrap_or_else(|| panic!("missing daemon source {suffix}"))
    };

    let app_state = source("app_state.rs");
    for retired_field in [
        "pub layouts:",
        "pub layouts_path:",
        "pub layout_auto_exclusions:",
        "pub layout_auto_exclusions_path:",
        "pub layout_mutation_test_hooks:",
    ] {
        assert!(!app_state.contains(retired_field), "found {retired_field}");
    }

    let discovery = source("discovery/mod.rs");
    assert!(discovery.contains("pub layout: LayoutContext"));
    for bypass in [
        "pub spatial_engine:",
        "pub scene_manager:",
        "pub layouts:",
        "pub layouts_path:",
        "pub layout_auto_exclusions:",
        "pub scene_transactions:",
    ] {
        assert!(
            !discovery.contains(bypass),
            "discovery runtime exposes {bypass}"
        );
    }

    let startup = source("startup/mod.rs");
    for bypass in [
        "pub layouts:",
        "pub layouts_path:",
        "pub layout_auto_exclusions:",
        "pub layout_auto_exclusions_path:",
    ] {
        assert!(!startup.contains(bypass), "daemon state exposes {bypass}");
    }

    let contexts = source("domain/context.rs");
    let device_context = contexts
        .split_once("pub struct DeviceContext")
        .map(|(_, tail)| tail)
        .expect("device context should exist");
    for layout_authority in [
        "layout_auto_exclusions",
        "resolved_layout_device_id",
        "layout_outputs_for",
        "connected_display_surface_layouts",
        "sync_connectivity",
        "reconcile_zone_auto_exclusions",
        "remove_zone_auto_exclusions",
    ] {
        assert!(
            !device_context.contains(layout_authority),
            "device context retains layout authority: {layout_authority}"
        );
    }

    let layout_domain = source("domain/layout.rs");
    assert!(!layout_domain.contains("catalog_for_test"));
    assert!(!layout_domain.contains("catalog_path_for_test"));
    assert!(!layout_domain.contains("OnceLock"));
    assert!(!layout_domain.contains("Weak<DaemonDriverHost>"));
    assert!(!layout_domain.contains("clippy::too_many_lines"));
    assert!(
        layout_domain
            .contains("#[cfg(feature = \"persistence-test-hooks\")]\npub struct LayoutTestFixture")
    );
    assert!(layout_domain.contains("pub(crate) struct LayoutRuntime"));

    for collaborator in [
        "domain/layout/auto_layout.rs",
        "domain/layout/catalog.rs",
        "domain/layout/convergence.rs",
        "domain/layout/exclusions.rs",
        "domain/layout/publication.rs",
        "domain/layout/workflows.rs",
    ] {
        let collaborator_source = source(collaborator);
        assert!(
            collaborator_source
                .lines()
                .all(|line| !line.starts_with("pub ") && !line.starts_with("pub(crate)")),
            "layout collaborator exports a top-level item: {collaborator}"
        );
    }

    let auto_layout = source("domain/layout/auto_layout.rs");
    assert!(auto_layout.contains("pub(super) fn append_auto_layout_zones_for_device"));
    assert!(auto_layout.contains("pub(super) fn reconcile_auto_layout_zones_for_device"));
    assert!(!auto_layout.contains("\npub fn append_auto_layout_zones_for_device"));
    assert!(!auto_layout.contains("\npub fn reconcile_auto_layout_zones_for_device"));

    let scene_transactions = source("scene_transactions.rs");
    assert!(!scene_transactions.contains("pub async fn apply_layout_update"));
    assert!(!scene_transactions.contains("pub async fn apply_prepared_layout_update_under_guard"));

    let adapter = source("api/layouts.rs");
    for bypass in [
        "state.layouts",
        "state.layouts_path",
        "state.spatial_engine",
        "state.scene_transactions",
        "state.layout_auto_exclusions",
    ] {
        assert!(
            !adapter.contains(bypass),
            "layout adapter contains {bypass}"
        );
    }
    assert!(adapter.contains("state.domains.layout"));
    assert!(!adapter.contains("pub use crate::domain::layout"));

    let websocket = source("api/ws/session.rs");
    assert!(!websocket.contains("crate::api::layouts"));
}
