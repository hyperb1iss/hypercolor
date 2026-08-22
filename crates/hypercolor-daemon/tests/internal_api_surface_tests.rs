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
fn transports_use_the_scene_domain_mutation_authority() {
    let offenders = daemon_sources()
        .into_iter()
        .filter(|(path, _)| {
            path.components()
                .any(|component| component.as_os_str() == "api" || component.as_os_str() == "mcp")
        })
        .filter(|(_, source)| source.contains("scene_manager.begin_mutation"))
        .map(|(path, _)| path.display().to_string())
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "transport adapters bypassed SceneContext:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn domain_modules_do_not_depend_on_transport_modules() {
    let offenders = daemon_sources()
        .into_iter()
        .filter(|(path, _)| {
            path.components()
                .any(|component| component.as_os_str() == "domain")
        })
        .filter_map(|(path, source)| {
            ["crate::api::", "crate::mcp::"]
                .into_iter()
                .find(|pattern| source.contains(pattern))
                .map(|pattern| format!("{} contains {pattern}", path.display()))
        })
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "domain modules depend on transport modules:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn effect_fallback_worker_uses_domain_authority() {
    let sources = daemon_sources();
    let source = |suffix: &str| {
        sources
            .iter()
            .find(|(path, _)| path.ends_with(suffix))
            .map(|(_, source)| source.as_str())
            .unwrap_or_else(|| panic!("missing daemon source {suffix}"))
    };
    let startup = source("startup/lifecycle.rs");
    let api_root = source("api/mod.rs");

    assert!(
        startup.contains("crate::domain::effect::apply_error_fallback"),
        "effect fallback worker must enter through the effect domain"
    );
    assert!(
        !startup.contains("crate::api::apply_effect_error_fallback"),
        "effect fallback worker must not call a REST adapter"
    );
    assert!(
        !api_root.contains("apply_effect_error_fallback"),
        "REST router module must not own effect fallback policy"
    );
}

#[test]
fn active_effect_queries_use_domain_authority() {
    let sources = daemon_sources();
    let source = |suffix: &str| {
        sources
            .iter()
            .find(|(path, _)| path.ends_with(suffix))
            .map(|(_, source)| source.as_str())
            .unwrap_or_else(|| panic!("missing daemon source {suffix}"))
    };
    let mcp_payload = source("mcp/payload.rs");
    let system_api = source("api/system.rs");
    let effect_api = source("api/effects.rs");

    for adapter in [mcp_payload, system_api] {
        assert!(adapter.contains(".active_primary_effect()"));
        assert!(!adapter.contains("api::effects::active_primary_effect"));
        assert!(!adapter.contains("api::effects::active_effect_metadata"));
    }
    assert!(!effect_api.contains("fn active_primary_effect"));
    assert!(!effect_api.contains("fn active_effect_metadata"));
}

#[test]
fn effect_registry_watcher_uses_domain_authority() {
    let sources = daemon_sources();
    let startup = sources
        .iter()
        .find(|(path, _)| path.ends_with("startup/lifecycle.rs"))
        .map(|(_, source)| source.as_str())
        .expect("startup lifecycle source should exist");

    assert!(startup.contains("crate::domain::effect::invalidate_active_zones"));
    assert!(!startup.contains("crate::api::effects::invalidate"));
    assert!(!startup.contains("invalidate_active_render_groups"));
}

#[test]
fn domain_errors_do_not_render_transport_responses() {
    let banned = [
        "use axum",
        "axum::",
        "IntoResponse for DomainError",
        "ApiErrorBody",
        "ResponseMeta",
        "StatusCode",
    ];
    let offenders = daemon_sources()
        .into_iter()
        .filter(|(path, _)| {
            path.components()
                .any(|component| component.as_os_str() == "domain")
        })
        .flat_map(|(path, source)| {
            let code = source
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            banned
                .into_iter()
                .filter(|pattern| code.contains(pattern))
                .map(|pattern| format!("{} contains {pattern}", path.display()))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "domain modules render transport responses:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn output_has_one_brightness_percentage_projection() {
    let definitions = daemon_sources()
        .into_iter()
        .filter(|(_, source)| {
            source.lines().any(|line| {
                let line = line.trim_start();
                line.starts_with("fn brightness_percent(")
                    || line.starts_with("pub(crate) fn brightness_percent(")
            })
        })
        .map(|(path, _)| path)
        .collect::<Vec<_>>();

    assert_eq!(definitions.len(), 1, "found {definitions:?}");
    assert!(definitions[0].ends_with("output_power.rs"));
}

#[test]
fn display_worker_delegates_delivery_policy_to_the_core_lane() {
    let worker = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/display_output/worker.rs"),
    )
    .expect("display worker source should read");

    assert!(worker.contains("output_lane.write("));
    assert!(worker.contains("lane_is_active"));
    for retired_pipeline in [
        "RetryAfterFailure",
        "retry_after",
        "schedule_display_retry",
        "schedule_cached_display_retry",
        "BackendIo",
    ] {
        assert!(
            !worker.contains(retired_pipeline),
            "daemon display worker retained {retired_pipeline}"
        );
    }

    let backend_io = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../hypercolor-core/src/device/manager/backend_io.rs"),
    )
    .expect("core backend I/O source should read");
    for retired_bypass in [
        "pub async fn write_display_frame(",
        "pub async fn write_display_frame_owned(",
        "pub async fn write_display_payload_owned(",
    ] {
        assert!(
            !backend_io.contains(retired_bypass),
            "BackendIo retained display bypass {retired_bypass}"
        );
    }
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

#[test]
fn layout_mutation_capabilities_have_named_visibility_boundaries() {
    let sources = daemon_sources();
    let source = |suffix: &str| {
        sources
            .iter()
            .find(|(path, _)| path.ends_with(suffix))
            .map(|(_, source)| source.as_str())
            .unwrap_or_else(|| panic!("missing daemon source {suffix}"))
    };

    let library = source("lib.rs");
    assert!(library.contains("pub(crate) mod scene_transactions;"));
    assert!(!library.contains("pub mod scene_transactions;"));
    assert!(library.contains(
        "#[cfg(feature = \"persistence-test-hooks\")]\n#[doc(hidden)]\npub use scene_transactions::{LayoutPublicationTestExecutor, LayoutTransactionRejection};"
    ));
    assert!(!library.contains("SceneTransactionConsumer"));
    assert!(library.contains("SceneTransactionQueue"));

    let layout_store = source("layout_store.rs");
    assert!(layout_store.contains("pub(crate) fn save("));
    assert!(!layout_store.contains("\npub fn save("));
    let exclusions = source("layout_auto_exclusions.rs");
    assert!(!exclusions.contains("fn save("));
    assert!(exclusions.contains("pub(crate) fn serialize("));

    let app_state = source("app_state.rs");
    assert!(app_state.contains("pub(crate) scene_transactions: SceneTransactionQueue"));
    assert!(!app_state.contains("pub scene_transactions: SceneTransactionQueue"));
    assert!(app_state.contains(
        "#[cfg(feature = \"persistence-test-hooks\")]\n    #[doc(hidden)]\n    #[must_use]\n    pub fn layout_publication_test_executor("
    ));

    let layout = source("domain/layout.rs");
    for gated_capability in [
        "#[cfg(feature = \"persistence-test-hooks\")]\n#[doc(hidden)]\npub struct LayoutTestWorkflows",
        "#[cfg(feature = \"persistence-test-hooks\")]\nimpl LayoutTestWorkflows",
        "#[cfg(feature = \"persistence-test-hooks\")]\n    #[allow(\n        clippy::too_many_arguments,\n        reason = \"the fixture mirrors the production composition boundary\"\n    )]\n    pub fn new_test_context(",
        "#[cfg(feature = \"persistence-test-hooks\")]\n    #[doc(hidden)]\n    #[must_use]\n    pub const fn test_workflows(",
        "#[cfg(feature = \"persistence-test-hooks\")]\n    #[doc(hidden)]\n    #[must_use]\n    pub fn layout_publication_test_executor(",
    ] {
        assert!(
            layout.contains(gated_capability),
            "layout test capability is not feature-gated: {gated_capability}"
        );
    }

    let transactions = source("scene_transactions.rs");
    assert!(transactions.contains(
        "#[cfg(feature = \"persistence-test-hooks\")]\n#[doc(hidden)]\npub struct LayoutPublicationTestExecutor"
    ));
    assert!(
        transactions.contains(
            "#[cfg(feature = \"persistence-test-hooks\")]\nimpl LayoutPublicationTestExecutor {\n    #[must_use]\n    pub(crate) fn new("
        )
    );
    assert!(transactions.contains("pub(crate) struct SceneTransactionConsumer"));
    assert!(!transactions.contains("\npub struct SceneTransactionConsumer"));
    assert!(transactions.contains("pub(crate) fn consumer(&self)"));
    assert!(!transactions.contains("pub fn consumer(&self)"));
    assert!(transactions.contains("pub(crate) fn close(&self)"));
    assert!(!transactions.contains("pub fn close(&self)"));
    assert!(transactions.contains("pub(crate) struct LayoutTransactionAuthority"));
    assert!(transactions.contains("\nstruct PreparedLayoutUpdate"));
    assert!(!transactions.contains("pub(crate) struct PreparedLayoutUpdate"));
    assert!(transactions.contains("pub(crate) fn drain(&self)"));
    assert!(!transactions.contains("pub fn drain(&self)"));
    assert!(transactions.contains("pub(crate) fn accept(self)"));
    assert!(!transactions.contains("\npub fn accept(self)"));
    for removed_bypass in [
        "accept_and_commit_for_test",
        "accept_and_publish_for_test",
        "apply_prepared_layout_update_under_guard",
        "publish_prepared_layout_activation",
    ] {
        assert!(
            !transactions.contains(removed_bypass),
            "scene transaction bypass remains: {removed_bypass}"
        );
    }
    assert!(transactions.contains("pub async fn execute_next_layout_publication("));
    assert!(transactions.contains("pub async fn execute_next_layout_publication_with_hook"));
    assert!(transactions.contains("pub fn reject_next_layout_publication"));

    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .expect("daemon manifest should read");
    for test_target in [
        "api_tests",
        "attachment_api_tests",
        "discovery_tests",
        "display_output_tests",
        "domain_scene_service_tests",
        "render_thread_tests",
        "simulator_tests",
    ] {
        let gate = format!(
            "[[test]]\nname = \"{test_target}\"\nrequired-features = [\"persistence-test-hooks\"]"
        );
        assert!(
            manifest.contains(&gate),
            "layout integration target is not feature-gated: {test_target}"
        );
    }
}
