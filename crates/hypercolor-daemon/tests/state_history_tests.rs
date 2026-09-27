//! Previous-good generations of the daemon's durable stores.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

use hypercolor_daemon::durable_stores::{DURABLE_STORES, StoreLocation, StoreOwner};
use hypercolor_daemon::runtime_state::{self, RuntimeSessionSnapshot};
use hypercolor_daemon::state_history::{
    self, HISTORY_EXCLUSIONS, HistoryCommand, StoreRoots, history_stores,
};
use hypercolor_types::config::DaemonConfig;
use serde_json::Value;

fn roots(directory: &Path) -> StoreRoots {
    StoreRoots {
        config_file: directory.join("config").join("hypercolor.toml"),
        config_dir: directory.join("config"),
        data_dir: directory.join("data"),
        state_dir: directory.join("state"),
    }
}

fn run(command: HistoryCommand, roots: &StoreRoots) -> anyhow::Result<String> {
    let mut out = Vec::new();
    state_history::run_command(command, roots, &DaemonConfig::default(), &mut out)?;
    Ok(String::from_utf8(out).expect("command output is UTF-8"))
}

fn generation_ids(roots: &StoreRoots, store: &str) -> Vec<u64> {
    let listing = run(
        HistoryCommand::List {
            store: Some(store.to_owned()),
            json: true,
        },
        roots,
    )
    .expect("list runs");
    let listing: Value = serde_json::from_str(&listing).expect("list prints JSON");
    listing[0]["generations"]
        .as_array()
        .expect("generations array")
        .iter()
        .map(|generation| generation["id"].as_u64().expect("numeric id"))
        .collect()
}

#[test]
fn every_daemon_store_is_covered_or_excluded_with_a_reason() {
    let directory = tempfile::tempdir().expect("tempdir");
    let roots = roots(directory.path());
    let covered: BTreeSet<_> = history_stores(&roots)
        .iter()
        .map(|store| store.name)
        .collect();
    let excluded: BTreeSet<_> = HISTORY_EXCLUSIONS.iter().map(|(name, _)| *name).collect();

    for store in DURABLE_STORES
        .iter()
        .filter(|store| store.owner == StoreOwner::Daemon)
    {
        assert!(
            covered.contains(store.name) != excluded.contains(store.name),
            "{} must be either covered or excluded, not both or neither",
            store.name
        );
        if matches!(store.location, StoreLocation::Directory(_)) {
            assert!(
                excluded.contains(store.name),
                "{} is a directory",
                store.name
            );
        }
    }
    for (name, reason) in HISTORY_EXCLUSIONS {
        assert!(
            DURABLE_STORES
                .iter()
                .any(|store| store.name == *name && store.owner == StoreOwner::Daemon),
            "{name} is excluded but is not a daemon store"
        );
        assert!(!reason.is_empty(), "{name} needs a reason");
    }
    for name in [
        "runtime-state",
        "scenes",
        "layouts",
        "device-settings",
        "device-aliases",
        "library",
        "display-preferences",
        "config",
    ] {
        assert!(covered.contains(name), "{name} keeps history");
    }
}

#[test]
fn history_lives_under_the_state_root_per_store() {
    let directory = tempfile::tempdir().expect("tempdir");
    let roots = roots(directory.path());
    assert_eq!(
        state_history::history_directory(&roots.state_dir, "scenes"),
        roots.state_dir.join("history").join("scenes")
    );
    let config = history_stores(&roots)
        .into_iter()
        .find(|store| store.name == "config")
        .expect("config is covered");
    assert_eq!(config.path, roots.config_file);
}

#[test]
fn an_emptied_runtime_session_restores_and_the_restore_is_undoable() {
    let directory = tempfile::tempdir().expect("tempdir");
    let roots = roots(directory.path());
    let path = roots.state_dir.join("runtime-state.json");

    // The owner's session, saved by an earlier daemon run.
    let good = RuntimeSessionSnapshot {
        active_scene_id: Some("default".to_owned()),
        active_layout_id: Some("desk".to_owned()),
        ..RuntimeSessionSnapshot::default()
    };
    runtime_state::save(&path, &good).expect("seed the good session");

    let covered = state_history::enable(&roots, 10, Duration::from_secs(30));
    assert!(covered.iter().any(|store| store.name == "runtime-state"));

    // A stray request empties the default scene.
    runtime_state::save(&path, &RuntimeSessionSnapshot::default()).expect("bad save");
    let ids = generation_ids(&roots, "runtime-state");
    assert_eq!(ids.len(), 1, "the good session is kept");

    let output = run(
        HistoryCommand::Restore {
            store: "runtime-state".to_owned(),
            generation: ids[0],
        },
        &roots,
    )
    .expect("restore runs");
    assert!(output.contains("Restored runtime-state"), "{output}");
    let restored = runtime_state::load(&path)
        .expect("restored session loads")
        .expect("restored session exists");
    assert_eq!(restored.active_scene_id.as_deref(), Some("default"));
    assert_eq!(restored.active_layout_id.as_deref(), Some("desk"));

    // The emptied session was kept by the restore, so it can be undone.
    let ids = generation_ids(&roots, "runtime-state");
    assert_eq!(ids.len(), 2);
    assert!(
        output.contains(&format!("generation {}", ids[1])),
        "{output}"
    );
}

#[test]
fn history_commands_explain_unknown_and_excluded_stores() {
    let directory = tempfile::tempdir().expect("tempdir");
    let roots = roots(directory.path());

    let excluded = run(
        HistoryCommand::List {
            store: Some("credentials".to_owned()),
            json: false,
        },
        &roots,
    )
    .expect_err("credentials keep no history");
    assert!(
        excluded
            .to_string()
            .contains("no history is kept for credentials"),
        "{excluded}"
    );

    let unknown = run(
        HistoryCommand::Restore {
            store: "nonsense".to_owned(),
            generation: 1,
        },
        &roots,
    )
    .expect_err("unknown store");
    assert!(
        unknown.to_string().contains("unknown store nonsense"),
        "{unknown}"
    );
}

#[test]
fn the_table_listing_names_every_covered_store() {
    let directory = tempfile::tempdir().expect("tempdir");
    let roots = roots(directory.path());
    let table = run(
        HistoryCommand::List {
            store: None,
            json: false,
        },
        &roots,
    )
    .expect("list runs");
    for store in history_stores(&roots) {
        assert!(
            table.contains(store.name),
            "{} missing from {table}",
            store.name
        );
    }
    assert!(table.contains("no generations kept yet"));
}

#[test]
fn discovery_last_seen_stamps_do_not_churn_alias_history() {
    let directory = tempfile::tempdir().expect("tempdir");
    let roots = roots(directory.path());
    let path = roots.state_dir.join("device-aliases.json");
    fs::create_dir_all(&roots.state_dir).expect("state dir");
    let aliases = |fingerprint: &str, last_seen: u64| {
        serde_json::json!({
            "schema_version": 2,
            "aliases": {
                "usb:1234:5678:serial": {
                    "source": "usb_serial",
                    "raw": "serial",
                    "fingerprint": fingerprint,
                    "first_seen_epoch_s": 1,
                    "last_seen_epoch_s": last_seen,
                }
            }
        })
        .to_string()
    };
    fs::write(&path, aliases("pinned", 100)).expect("seed aliases");
    state_history::enable(&roots, 10, Duration::ZERO);

    for scan in 1..=20 {
        hypercolor_daemon::persistence::write_atomic(
            &path,
            aliases("pinned", 100 + scan).as_bytes(),
        )
        .expect("scan stamp");
    }
    assert!(
        generation_ids(&roots, "device-aliases").is_empty(),
        "last-seen stamps alone keep nothing"
    );

    hypercolor_daemon::persistence::write_atomic(&path, aliases("repinned", 200).as_bytes())
        .expect("real change");
    assert_eq!(generation_ids(&roots, "device-aliases").len(), 1);
    assert!(state_history::same_device_aliases(
        aliases("x", 1).as_bytes(),
        aliases("x", 2).as_bytes()
    ));
    assert!(!state_history::same_device_aliases(
        aliases("x", 1).as_bytes(),
        aliases("y", 1).as_bytes()
    ));
}

#[test]
fn zero_generations_turns_history_off() {
    let directory = tempfile::tempdir().expect("tempdir");
    let roots = roots(directory.path());
    let path = roots.data_dir.join("layouts.json");
    fs::create_dir_all(&roots.data_dir).expect("data dir");
    fs::write(&path, "[]").expect("seed layouts");

    assert!(state_history::enable(&roots, 0, Duration::ZERO).is_empty());
    hypercolor_daemon::persistence::write_atomic(&path, b"[1]").expect("write layouts");
    assert!(
        !state_history::history_directory(&roots.state_dir, "layouts").exists(),
        "no history directory appears"
    );
}
