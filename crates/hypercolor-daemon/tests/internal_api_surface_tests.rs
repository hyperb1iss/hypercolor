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
        })
        .map(|(path, _)| path.display().to_string())
        .collect::<Vec<_>>();
    assert!(
        bypasses.is_empty(),
        "runtime-state writers bypassed RuntimeSessionService:\n{}",
        bypasses.join("\n")
    );
    let source = |suffix: &str| {
        sources
            .iter()
            .find(|(path, _)| path.ends_with(suffix))
            .map(|(_, source)| source.as_str())
            .unwrap_or_else(|| panic!("missing daemon source {suffix}"))
    };
    let layout = source("domain/layout.rs");
    assert!(layout.contains("persist_snapshot_with"));
    assert!(!layout.contains("runtime_state::reserve_save"));
    assert!(!layout.contains("runtime_state::save_reserved"));

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
    assert!(effect_domain.contains("ctx.admit(&command.effect).await?"));
    assert!(!effect_domain.contains("pub async fn admit_generation"));

    let display_domain = source("domain/display.rs");
    assert!(display_domain.contains("pub effect: ResolvedEffect"));
    assert!(display_domain.contains("ctx.admit(&command.effect).await?"));

    let effect_api = source("api/effects.rs");
    assert!(effect_api.contains("resolve_for_mutation(&id)"));
    let display_api = source("api/displays.rs");
    assert!(display_api.contains("resolve_for_mutation(&body.effect_id)"));
    assert!(
        display_api
            .contains("apply_display_preference_overlay_admitted(state.as_ref(), device_id)")
    );
    let mcp_tools = source("mcp/tools/mod.rs");
    assert!(mcp_tools.contains("all_for_mutation()"));
    let mcp_displays = source("mcp/tools/displays.rs");
    assert!(mcp_displays.contains("apply_display_preference_overlay_admitted(state, device_id)"));

    let layer_domain = source("domain/layer.rs");
    assert!(layer_domain.contains("admit_layer_sources"));
    let scene_tree_domain = source("domain/scene_tree.rs");
    assert!(scene_tree_domain.contains("admit_layer_sources"));
    let scene_domain = source("domain/scene.rs");
    assert!(scene_domain.contains("admit_layer_sources"));

    let scene_api = source("api/scene.rs");
    assert!(scene_api.contains("insert_layer(&state.domains.effects"));
    assert!(!scene_api.contains("insert_layer(&state.domains.scene"));

    let admission = display_api
        .find("let _effect_admission = state.domains.effects.admit_current().await;")
        .expect("display overlay should acquire effect admission");
    let preference = display_api[admission..]
        .find("state.display_preferences.read().await")
        .expect("display overlay should read the admitted preference");
    let scene_commit = display_api[admission..]
        .find("domain::display::set_default_display_overlay")
        .expect("display overlay should commit under effect admission");
    assert!(preference < scene_commit);
}
