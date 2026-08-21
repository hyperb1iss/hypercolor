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
