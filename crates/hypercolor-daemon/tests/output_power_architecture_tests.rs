use std::path::{Path, PathBuf};

fn rust_sources(root: &Path) -> Vec<(PathBuf, String)> {
    let mut pending = vec![root.to_path_buf()];
    let mut sources = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("source directory should read") {
            let path = entry.expect("source entry should read").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let source = std::fs::read_to_string(&path).expect("Rust source should read");
                sources.push((path, source));
            }
        }
    }
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    sources
}

#[test]
fn app_and_daemon_state_expose_only_the_output_power_authority() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let app_state = std::fs::read_to_string(source_root.join("app_state.rs"))
        .expect("API state source should read");
    let daemon_state = std::fs::read_to_string(source_root.join("startup/mod.rs"))
        .expect("daemon state source should read");

    for (name, source) in [("AppState", app_state), ("DaemonState", daemon_state)] {
        assert!(
            source.contains("pub output_power: OutputPower"),
            "{name} must expose the canonical OutputPower handle"
        );
        assert!(
            source.contains("pub device_settings: DeviceSettingsAccess"),
            "{name} must expose only the non-brightness settings capability"
        );
        assert!(
            !source.contains("Arc<RwLock<DeviceSettingsStore>>"),
            "{name} must not expose the raw device settings store"
        );
        assert!(
            !source.contains("pub power_state:"),
            "{name} must not expose the raw watch sender"
        );
        assert!(
            !source.contains("pub output_power_transition:"),
            "{name} must not expose the transition mutex"
        );
    }
}

#[test]
fn control_plane_modules_do_not_bypass_output_power() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    for directory in ["api", "domain", "mcp", "startup"] {
        for (path, source) in rust_sources(&source_root.join(directory)) {
            if source.contains(".power_state") || source.contains("output_power_transition") {
                offenders.push(path);
            }
        }
    }
    let session = std::fs::read_to_string(source_root.join("session.rs"))
        .expect("session source should read");
    if session.contains(".power_state") || session.contains("output_power_transition") {
        offenders.push(source_root.join("session.rs"));
    }

    assert!(
        offenders.is_empty(),
        "control-plane modules must use OutputPower instead of raw state: {offenders:#?}"
    );
}

#[test]
fn legacy_session_power_functions_stay_deleted() {
    let session =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/session.rs"))
            .expect("session source should read");
    let legacy_functions = [
        "fn set_global_brightness(",
        "fn set_manual_pause(",
        "fn restore_manual_pause(",
        "fn set_output_stopped(",
        "fn clear_output_override(",
        "fn current_global_brightness(",
        "fn current_power_state(",
        "fn advance_transition_generation(",
        "fn update_power_state_for_generation(",
        "fn update_power_state_with_events_for_generation(",
        "fn update_power_state(",
        "fn update_power_state_with_events(",
    ];

    let offenders = legacy_functions
        .into_iter()
        .filter(|function| session.contains(function))
        .collect::<Vec<_>>();
    assert!(
        offenders.is_empty(),
        "session power helpers must remain OutputPower methods: {offenders:#?}"
    );
}

#[test]
fn transition_mutex_and_brightness_store_mutator_are_private() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let output_power = std::fs::read_to_string(source_root.join("output_power.rs"))
        .expect("output power source should read");
    let settings = std::fs::read_to_string(source_root.join("device_settings.rs"))
        .expect("device settings source should read");

    assert!(output_power.contains("transition: Mutex<()>"));
    assert!(!output_power.contains("pub transition: Mutex<()>"));
    assert!(output_power.contains("settings: Arc<RwLock<DeviceSettingsStore>>"));
    assert!(!output_power.contains("pub settings: Arc<RwLock<DeviceSettingsStore>>"));
    assert!(settings.contains("pub(crate) fn persist_global_brightness("));
    assert!(!settings.contains("pub fn persist_global_brightness("));
    assert!(settings.contains("&crate::output_power::BrightnessMutationAuthority"));
    assert!(output_power.contains("brightness_authority: BrightnessMutationAuthority"));
    assert!(!output_power.contains("pub brightness_authority: BrightnessMutationAuthority"));
    assert!(settings.contains("pub struct DeviceSettingsAccess"));
    assert!(settings.contains("store: Arc<RwLock<DeviceSettingsStore>>"));
    assert!(!settings.contains("pub store: Arc<RwLock<DeviceSettingsStore>>"));
    assert!(!settings.contains("pub async fn read("));
    assert!(!settings.contains("pub async fn write("));
}

#[test]
fn brightness_mutation_and_event_publication_stay_inside_output_power() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    for (path, source) in rust_sources(&source_root) {
        if path.ends_with("output_power.rs") || path.ends_with("device_settings.rs") {
            continue;
        }
        if source.contains("persist_global_brightness(")
            || source.contains("HypercolorEvent::BrightnessChanged")
            || source.contains("watch::Sender<OutputPowerState>")
            || source.contains("Arc<RwLock<DeviceSettingsStore>>")
        {
            offenders.push(path);
        }
    }
    assert!(
        offenders.is_empty(),
        "brightness mutation capabilities escaped their owners: {offenders:#?}"
    );
}

#[test]
fn raw_output_power_mutators_stay_private() {
    let output_power =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/output_power.rs"))
            .expect("output power source should read");

    for mutator in [
        "update_for_generation",
        "update_with_events_for_generation",
        "update_state",
        "update_state_with_observer",
        "update_state_with_events",
    ] {
        assert!(
            !output_power.contains(&format!("pub(crate) fn {mutator}"))
                && !output_power.contains(&format!("pub fn {mutator}")),
            "raw OutputPower mutator {mutator} must stay private"
        );
    }
}

#[test]
fn runtime_state_never_serializes_or_restores_brightness() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let runtime_state = std::fs::read_to_string(source_root.join("runtime_state.rs"))
        .expect("runtime state source should read");
    let startup = std::fs::read_to_string(source_root.join("startup/lifecycle.rs"))
        .expect("startup lifecycle source should read");
    let runtime_session = std::fs::read_to_string(source_root.join("domain/context.rs"))
        .expect("runtime session source should read");

    assert!(!runtime_state.contains("pub global_brightness:"));
    assert!(!startup.contains("snapshot.global_brightness"));
    assert!(!runtime_session.contains("snapshot.global_brightness"));
}

#[test]
fn mcp_brightness_uses_the_serialized_mutation_receipt() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mcp = std::fs::read_to_string(source_root.join("mcp/tools/devices.rs"))
        .expect("MCP device tools source should read");
    let compact = mcp
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();

    assert!(!mcp.contains("output_power.global_brightness()"));
    assert!(compact.contains("outcome.previous_brightness"));
}
