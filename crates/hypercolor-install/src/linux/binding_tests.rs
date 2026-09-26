use std::fs::{self, File};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::net::UnixListener;
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::*;
use crate::{
    LINUX_DIRECTORY_ITEMS, LINUX_LAYOUT_ITEMS, LinuxDirectoryState, LinuxNativeExecutor,
    LinuxSystemdConnection, PlatformTransactionRecord, UnitId, copy_installed_release_unit,
    stage_release_payload,
};

#[test]
fn native_command_constructor_binds_initial_and_recorded_prior_exactly_once() {
    let home =
        tempfile::tempdir_in(std::env::var_os("HOME").expect("owned home")).expect("isolated home");
    let old = InstallStore::new(home.path().join(".local/lib/hypercolor"), 65536);
    let old_lock = old.acquire_anchored_lock(home.path()).expect("old lock");
    let source = home.path().join("source");
    fs::create_dir(&source).expect("source");
    let executable = write_release(&source);
    let id = UnitId::new(digest(
        &fs::read(source.join("manifest.json")).expect("manifest"),
    ))
    .expect("id");
    let original = stage_release_payload(&old, &old_lock, &source, &executable, &id)
        .expect("original release");
    let store = InstallStore::with_roots(
        home.path().join("release"),
        home.path().join("state"),
        65536,
    )
    .expect("split store");
    let lock = store
        .acquire_anchored_lock(home.path())
        .expect("state lock");
    let copied = copy_installed_release_unit(&store, &lock, &original).expect("copied release");
    assert_eq!(original.id(), copied.id());
    assert_ne!(original, copied);
    let runtime = home.path().join("runtime");
    fs::create_dir(&runtime).expect("runtime");
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).expect("private runtime");
    fs::create_dir(runtime.join("systemd")).expect("manager directory");
    fs::set_permissions(runtime.join("systemd"), fs::Permissions::from_mode(0o755))
        .expect("manager directory mode");
    let _manager = UnixListener::bind(runtime.join("systemd/private")).expect("manager socket");
    let connection = LinuxSystemdConnection::from_runtime_directory(
        &runtime,
        fs::metadata(&runtime).expect("owner").uid(),
    )
    .expect("connection authority");
    let record = record(&store, &copied, &old, &original);
    // Receipt-only and journal-backed resumes feed the same exact durable record.
    for durable_record in [None, Some(&record), Some(&record)] {
        let tree = LinuxPublicTree::new(&lock, home.path()).expect("public tree");
        let executor = LinuxNativeExecutor::new_with_connection(
            &store,
            &lock,
            tree,
            "127.0.0.1:9420".parse().expect("loopback"),
            connection.clone(),
        )
        .expect("native executor");
        let platform = bind_platform(
            home.path(),
            &store,
            vec![copied.clone()],
            executor,
            durable_record,
            Some(&original),
            std::time::Duration::ZERO,
            None,
        )
        .expect("single native prior binding");
        drop(platform);
    }
    writable_directories(home.path());
}

fn record(
    store: &InstallStore,
    copied: &UnitRecord,
    old: &InstallStore,
    original: &UnitRecord,
) -> PlatformTransactionRecord {
    let paths = [
        "bin/hypercolor",
        "bin/hypercolor-daemon",
        "bin/hypercolor-app",
        "bin/hypercolor-tui",
        "bin/hypercolor-open",
        "share/applications/hypercolor.desktop",
        "share/bash-completion/completions/hypercolor",
        "share/zsh/site-functions/_hypercolor",
        "share/fish/vendor_completions.d/hypercolor.fish",
        "share/icons/hicolor/48x48/apps/hypercolor.png",
        "share/icons/hicolor/128x128/apps/hypercolor.png",
        "share/icons/hicolor/256x256/apps/hypercolor.png",
    ];
    let directories: std::collections::BTreeMap<_, _> = LINUX_DIRECTORY_ITEMS
        .into_iter()
        .map(|item| (item, LinuxDirectoryState::Present))
        .collect();
    let layout: Vec<_> = LINUX_LAYOUT_ITEMS
        .into_iter()
        .zip(paths)
        .map(|(item, path)| {
            json!({"effect":{"kind":"entry", "item":item,"prior":{"kind":"absent"},
            "candidate_target":store.active_path().join(path)}})
        })
        .collect();
    PlatformTransactionRecord::linux(
        1,
        serde_json::to_vec(&json!({
            "candidate":binding(copied,store), "prior":binding(original,old),
            "baseline_systemd":{"load_state":"not-found","active_state":"inactive",
                "sub_state":"dead","unit_file_state":"","fragment_path":"",
                "exec_start":"","main_pid":0,"invocation_id":""},
            "prior_launcher":{"kind":"absent"},"prior_launcher_bytes":[],
            "candidate_launcher":null,"prior_directories":directories,"layout":layout,
            "first_conversion":false
        }))
        .expect("complete record"),
    )
    .expect("record bound")
}

fn binding(unit: &UnitRecord, store: &InstallStore) -> Value {
    let path = store.unit_path(unit.id()).join("bin/hypercolor-daemon");
    let metadata = fs::metadata(&path).expect("daemon");
    json!({"unit":unit.id(),"daemon_path":path,"daemon_sha256":digest(b"daemon"),
        "daemon_size":metadata.len(),"daemon_device":metadata.dev(),
        "daemon_inode":metadata.ino(),"version":"9.8.7"})
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn writable_directories(path: &Path) {
    if !fs::symlink_metadata(path).expect("metadata").is_dir() {
        return;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("cleanup mode");
    for entry in fs::read_dir(path).expect("directory") {
        writable_directories(&entry.expect("entry").path());
    }
}

fn write_release(root: &Path) -> File {
    let version = "9.8.7";
    let daemon = b"daemon".as_slice();
    let directories = [
        "bin",
        "share",
        "share/hypercolor",
        "share/hypercolor/ui",
        "share/hypercolor/effects",
        "share/hypercolor/effects/bundled",
        "share/hypercolor/docs",
        "share/hypercolor/agents",
        "share/hypercolor/agents/skills",
        "share/hypercolor/agents/agents",
        "share/hypercolor/skills",
        "share/hypercolor/site",
    ];
    let files = [
        ("bin/hypercolor-daemon", daemon),
        ("bin/hypercolor", b"candidate".as_slice()),
        ("bin/hypercolor-app", b"app".as_slice()),
        ("bin/hypercolor-tui", b"tui".as_slice()),
        ("bin/hypercolor-open", b"open".as_slice()),
        ("share/hypercolor/ui/index.html", b"ui".as_slice()),
        (
            "share/hypercolor/effects/bundled/effect.html",
            b"effect".as_slice(),
        ),
        (
            "share/hypercolor/agents/skills/skill.md",
            b"skill".as_slice(),
        ),
        (
            "share/hypercolor/agents/agents/agent.md",
            b"agent".as_slice(),
        ),
        ("share/hypercolor/skills/skill.md", b"user skill".as_slice()),
    ];
    let mut members = Vec::new();
    for directory in directories {
        fs::create_dir_all(root.join(directory)).expect("directory");
        fs::set_permissions(root.join(directory), fs::Permissions::from_mode(0o755)).expect("mode");
        members.push(json!({"path":directory,"type":"directory","mode":0o755}));
    }
    for (path, bytes) in files {
        fs::write(root.join(path), bytes).expect("file");
        let mode = if path.starts_with("bin/") {
            0o755
        } else {
            0o644
        };
        fs::set_permissions(root.join(path), fs::Permissions::from_mode(mode)).expect("mode");
        members.push(json!({
            "path":path,"type":"file","mode":mode,"size":bytes.len(),"sha256":digest(bytes)
        }));
    }
    members.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let manifest = serde_json::to_vec_pretty(&json!({
        "name":"hypercolor","version":version,"platform":"linux-x86_64",
        "rust_target":"x86_64-unknown-linux-gnu",
        "binaries":["hypercolor-daemon","hypercolor","hypercolor-app","hypercolor-tui","hypercolor-open"],
        "assets":{"ui_files":1,"bundled_effect_files":1,"docs_files":0,"skill_files":1,"user_skill_files":1,"agent_files":1,"site_files":0},
        "managed_package":{
            "schema_version":1,"owner":"linux-user-tarball","launcher_contract":1,
            "components":{"daemon":"bin/hypercolor-daemon","cli":"bin/hypercolor",
                "ui":"share/hypercolor/ui","bundled_effects":"share/hypercolor/effects/bundled"},
            "compatibility":{"stores":[{"name":"config","storage_format":"toml",
                "readable_schema_min":4,"readable_schema_max":5,"written_schema":5,
                "migration_mode":"backward_compatible"}]},
        },
        "members":members,
    }))
    .expect("manifest JSON");
    fs::write(root.join("manifest.json"), manifest).expect("manifest");
    fs::set_permissions(
        root.join("manifest.json"),
        fs::Permissions::from_mode(0o644),
    )
    .expect("manifest mode");
    File::open(root.join("bin/hypercolor")).expect("candidate")
}
