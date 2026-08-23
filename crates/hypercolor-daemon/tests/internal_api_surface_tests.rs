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
            } else if path.extension().is_some_and(|extension| extension == "rs")
                && !path.ends_with("tests.rs")
            {
                let source = std::fs::read_to_string(&path)
                    .expect("daemon Rust source should read as UTF-8");
                sources.push((path, source));
            }
        }
    }
    sources
}

fn references_rust_member(source: &str, member: &str) -> bool {
    source.match_indices(member).any(|(start, _)| {
        let prefix = source[..start].trim_end();
        let suffix = source[start + member.len()..].trim_start();
        (prefix.ends_with("SceneStore::") || prefix.ends_with('.')) && suffix.starts_with('(')
    })
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
fn output_static_hold_lifecycle_uses_output_context_directly() {
    let sources = daemon_sources();
    let lifecycle = sources
        .iter()
        .find(|(path, _)| path.ends_with("startup/lifecycle.rs"))
        .map(|(_, source)| source.as_str())
        .expect("startup lifecycle source should exist");
    let worker = lifecycle
        .split("fn spawn_output_static_hold_worker")
        .nth(1)
        .and_then(|source| source.split("fn spawn_effect_error_fallback_worker").next())
        .expect("static hold worker should remain structurally visible");

    assert!(lifecycle.contains("self.domains.output.reconcile_static_hold().await;"));
    assert!(worker.contains("let output = self.domains.output.clone();"));
    assert!(!worker.contains("AppState::from_daemon_state"));
    assert!(!lifecycle.contains("startup_output_state"));
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

    assert!(startup.contains("use crate::domain::effect::reload_registry_file;"));
    assert!(startup.contains("reload_registry_file(&watcher_state"));
    assert!(!startup.contains("crate::api::effects::invalidate"));
    assert!(!startup.contains("reload_single(&path)"));
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

#[test]
fn registry_refreshes_share_the_effect_domain_identity_authority() {
    let sources = daemon_sources();
    let source = |suffix: &str| {
        sources
            .iter()
            .find(|(path, _)| path.ends_with(suffix))
            .map(|(_, source)| source.as_str())
            .unwrap_or_else(|| panic!("missing daemon source {suffix}"))
    };
    let effect_api = source("api/effects.rs");
    assert!(effect_api.contains("domain::effect::rescan_registry"));
    assert!(effect_api.contains("domain::effect::install_registry_file"));
    assert!(!effect_api.contains("domains.effects.rescan()"));
    assert!(!effect_api.contains("domains.effects.register("));

    let lifecycle = source("startup/lifecycle.rs");
    assert!(lifecycle.contains("use crate::domain::effect::reload_registry_file;"));
    assert!(lifecycle.contains("reload_registry_file(&watcher_state"));
    assert!(!lifecycle.contains("reload_single(&path)"));

    let app_state = source("app_state.rs");
    assert!(app_state.contains("playlist_runtime: Arc::clone(&daemon.playlist_runtime)"));
    let startup = source("startup/mod.rs");
    assert!(startup.contains("pub playlist_runtime: Arc<Mutex<PlaylistRuntimeState>>"));
    let library = source("lib.rs");
    assert!(!library.contains("mod effect_id_migration"));
}

#[test]
fn runtime_state_writers_share_the_runtime_session_authority() {
    let sources = daemon_sources();
    let bypasses = sources
        .iter()
        .filter(|(path, _)| {
            !path.ends_with("domain/context.rs")
                && !path.ends_with("runtime_state.rs")
                && !path.ends_with("startup/services.rs")
        })
        .filter(|(_, source)| {
            source.contains("runtime_state::reserve_save")
                || source.contains("runtime_state::save_reserved")
                || source.contains("runtime_state::save(")
        })
        .map(|(path, _)| path.display().to_string())
        .collect::<Vec<_>>();
    assert!(
        bypasses.is_empty(),
        "runtime-state writers bypassed RuntimeSessionProjection:\n{}",
        bypasses.join("\n")
    );
    let source = |suffix: &str| {
        sources
            .iter()
            .find(|(path, _)| path.ends_with(suffix))
            .map(|(_, source)| source.as_str())
            .unwrap_or_else(|| panic!("missing daemon source {suffix}"))
    };
    let layout_publication = source("domain/layout/publication.rs");
    assert!(layout_publication.contains("persist_snapshot_with"));
    assert!(!layout_publication.contains("runtime_state::reserve_save"));
    assert!(!layout_publication.contains("runtime_state::save_reserved"));

    let lifecycle = source("startup/lifecycle.rs");
    assert!(lifecycle.contains(".runtime_session\n            .persist_snapshot()"));
    assert!(!lifecycle.contains("runtime_state::reserve_save"));
    assert!(!lifecycle.contains("runtime_state::save_reserved"));
}

#[test]
fn scene_store_writers_share_the_scene_service_authority() {
    assert!(references_rust_member(
        "snapshot_alias . reserve_save (scenes)",
        "reserve_save"
    ));
    assert!(references_rust_member(
        "snapshot_alias.save_reserved(pending)",
        "save_reserved"
    ));

    let sources = daemon_sources();
    let raw_writer_members = [
        "reserve_save",
        "save_reserved",
        "save_reserved_stage_aware",
        "replace_named_scenes",
        "persist_normalization",
        "kick_persistence",
        "sync_from_manager",
    ];
    let bypasses = sources
        .iter()
        .filter(|(path, _)| {
            !path.ends_with("domain/scene.rs")
                && !path.ends_with("profile_import.rs")
                && !path.ends_with("scene_store.rs")
                && !path.ends_with("startup/services.rs")
        })
        .flat_map(|(path, source)| {
            raw_writer_members
                .into_iter()
                .filter(|member| references_rust_member(source, member))
                .map(|member| format!("{} references {member}", path.display()))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert!(
        bypasses.is_empty(),
        "scene-store writers bypassed SceneService:\n{}",
        bypasses.join("\n")
    );

    let source = |suffix: &str| {
        sources
            .iter()
            .find(|(path, _)| path.ends_with(suffix))
            .map(|(_, source)| source.as_str())
            .unwrap_or_else(|| panic!("missing daemon source {suffix}"))
    };
    let lifecycle = source("startup/lifecycle.rs");
    assert!(lifecycle.contains("persist_scene_store_snapshot(&self.scene_manager)"));
    assert!(!lifecycle.contains("self.scene_store"));
    let watcher_shutdown = lifecycle
        .split_once("if let Some(handle) = self.effect_watcher_task.take()")
        .map(|(_, shutdown)| shutdown)
        .expect("shutdown should stop the effect watcher");
    let watcher_shutdown = watcher_shutdown
        .split_once("if let Some(handle) = self.display_preference_sync_task.take()")
        .map(|(shutdown, _)| shutdown)
        .expect("watcher shutdown should precede preference sync shutdown");
    assert!(watcher_shutdown.contains("handle.abort()"));
    assert!(watcher_shutdown.contains("handle.await"));

    let app_state = source("app_state.rs");
    assert!(!app_state.contains("pub scene_store:"));
    assert!(!app_state.contains("Arc::new(RwLock::new(scene_store))"));
    let startup = source("startup/mod.rs");
    assert!(!startup.contains("pub scene_store:"));

    let scene_store = source("scene_store.rs");
    assert!(scene_store.contains("pub(crate) struct SceneStore"));
    assert!(!scene_store.contains("pub struct SceneStore {"));
    assert!(!scene_store.contains("derive(Debug, Clone)\npub(crate) struct SceneStore"));
    for public_writer in [
        "pub fn new(",
        "pub fn save(",
        "pub fn reserve_save(",
        "pub fn save_reserved(",
        "pub fn replace_named_scenes(",
        "pub fn migrate_effect_ids(",
    ] {
        assert!(
            !scene_store.contains(public_writer),
            "SceneStore retained public writer {public_writer}"
        );
    }

    let scene_domain = source("domain/scene.rs");
    assert!(scene_domain.contains("store: SceneStore"));
    assert!(scene_domain.contains("Some(Arc::new(tokio::sync::RwLock::new(store)))"));
    assert!(!scene_domain.contains("store.read().await.clone()"));

    let raw_constructors = sources
        .iter()
        .filter(|(path, _)| {
            !path.ends_with("app_state.rs")
                && !path.ends_with("startup/services.rs")
                && !path.ends_with("profile_import.rs")
                && !path.ends_with("scene_store.rs")
        })
        .filter(|(_, source)| {
            source.contains("SceneStore::new(") || source.contains("SceneStore::load(")
        })
        .map(|(path, _)| path.display().to_string())
        .collect::<Vec<_>>();
    assert!(
        raw_constructors.is_empty(),
        "scene-store capability escaped startup ownership:\n{}",
        raw_constructors.join("\n")
    );

    let library = source("lib.rs");
    assert!(library.contains("pub(crate) mod profile_import;"));
    assert!(!library.contains("pub mod profile_import;"));
    let profile_import = source("profile_import.rs");
    assert!(profile_import.contains("pub(crate) fn import_profiles("));
    assert!(!profile_import.contains("pub fn import_profiles("));
    let profile_import_callers = sources
        .iter()
        .filter(|(path, _)| !path.ends_with("startup/services.rs"))
        .filter(|(_, source)| source.contains("profile_import::import_profiles("))
        .map(|(path, _)| path.display().to_string())
        .collect::<Vec<_>>();
    assert!(
        profile_import_callers.is_empty(),
        "profile import escaped startup ownership:\n{}",
        profile_import_callers.join("\n")
    );
}

#[test]
fn effect_mutations_require_generation_qualified_admission() {
    let sources = daemon_sources();
    let source = |suffix: &str| {
        sources
            .iter()
            .find(|(path, _)| path.ends_with(suffix))
            .map(|(_, source)| source.as_str())
            .unwrap_or_else(|| panic!("missing daemon source {suffix}"))
    };
    let effect_domain = source("domain/effect.rs");
    assert!(effect_domain.contains("pub effect: ResolvedEffect"));
    assert!(effect_domain.contains(".admit_resolved_controls(command.effect, &command.controls)"));
    assert!(effect_domain.contains("pub(crate) struct AdmittedEffectControls"));
    assert!(!effect_domain.contains("already normalized against the effect's schema"));
    assert!(!effect_domain.contains("pub async fn admit_generation"));

    let display_domain = source("domain/display.rs");
    assert!(display_domain.contains("pub effect: ResolvedEffect"));
    assert!(display_domain.contains("admit_display_face_controls"));
    assert!(display_domain.contains("admit_current_display_face_controls"));

    let effect_api = source("api/effects.rs");
    assert!(effect_api.contains("resolve_for_mutation(&id)"));
    let display_api = source("api/displays.rs");
    assert!(display_api.contains("resolve_for_mutation(&body.effect_id)"));
    assert!(
        display_api.contains("apply_display_preference_overlay_checked(state.as_ref(), device_id)")
    );
    let mcp_tools = source("mcp/tools/mod.rs");
    assert!(mcp_tools.contains("all_for_mutation()"));
    let mcp_displays = source("mcp/tools/displays.rs");
    assert!(mcp_displays.contains("apply_display_preference_overlay_checked(state, device_id)"));

    let layer_domain = source("domain/layer.rs");
    assert!(layer_domain.contains("admit_layer_sources"));
    let scene_tree_domain = source("domain/scene_tree.rs");
    assert!(scene_tree_domain.contains("admit_layer_sources"));
    let scene_domain = source("domain/scene.rs");
    assert!(scene_domain.contains("admit_layer_sources"));

    let scene_api = source("api/scene.rs");
    assert!(scene_api.contains("insert_layer(&state.domains.effects"));
    assert!(!scene_api.contains("insert_layer(&state.domains.scene"));

    let preference = display_api
        .find("state.display_preferences.read().await")
        .expect("display overlay should read the preference");
    let admission = display_api[preference..]
        .find("admit_current_display_face_controls")
        .expect("display overlay should admit preference controls");
    let scene_commit = display_api[preference..]
        .find("domain::display::set_default_display_overlay")
        .expect("display overlay should commit under effect admission");
    assert!(admission < scene_commit);
}
