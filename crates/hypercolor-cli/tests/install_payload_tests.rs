#![cfg(unix)]

use std::fs::{self, File};
use std::io::Read as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hypercolor_cli::install::{
    InstallStore, MAX_RELEASE_MANIFEST_BYTES, ReleasePayloadError, stage_release_payload,
    stage_release_payload_from_authority,
};
use hypercolor_platform_fs::{DirectoryEntryKind, ReadOnlyDirectoryAuthority};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

const BINARIES: [&str; 5] = [
    "hypercolor-daemon",
    "hypercolor",
    "hypercolor-app",
    "hypercolor-tui",
    "hypercolor-open",
];

struct ReleaseFixture {
    root: tempfile::TempDir,
    candidate: File,
    manifest: Vec<u8>,
}

impl ReleaseFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("release root");
        let (candidate, manifest) = write_release(root.path());
        Self {
            root,
            candidate,
            manifest,
        }
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn manifest_value(&self) -> Value {
        serde_json::from_slice(&fs::read(self.path().join("manifest.json")).expect("read manifest"))
            .expect("decode manifest")
    }

    fn write_manifest(&self, value: &Value) {
        fs::write(
            self.path().join("manifest.json"),
            serde_json::to_vec_pretty(value).expect("encode manifest"),
        )
        .expect("rewrite manifest");
    }
}

fn write_release(root: &Path) -> (File, Vec<u8>) {
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
        "share/hypercolor/site",
    ];
    let files = [
        ("bin/hypercolor-daemon", b"daemon".as_slice()),
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
    ];
    let mut members = Vec::new();
    for directory in directories {
        fs::create_dir_all(root.join(directory)).expect("create release directory");
        fs::set_permissions(root.join(directory), fs::Permissions::from_mode(0o755))
            .expect("set release directory mode");
        members.push(json!({"path": directory, "type": "directory", "mode": 0o755}));
    }
    for (path, bytes) in files {
        fs::write(root.join(path), bytes).expect("write release file");
        let mode = if path.starts_with("bin/") {
            0o755
        } else {
            0o644
        };
        fs::set_permissions(root.join(path), fs::Permissions::from_mode(mode))
            .expect("set release file mode");
        members.push(json!({
            "path": path,
            "type": "file",
            "mode": mode,
            "size": bytes.len(),
            "sha256": sha256(bytes),
        }));
    }
    members.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let manifest = serde_json::to_vec_pretty(&json!({
        "name": "hypercolor",
        "version": "0.3.2",
        "platform": "macos-arm64",
        "rust_target": "aarch64-apple-darwin",
        "binaries": BINARIES,
        "assets": {
            "ui_files": 1,
            "bundled_effect_files": 1,
            "docs_files": 0,
            "skill_files": 1,
            "agent_files": 1,
            "site_files": 0,
        },
        "members": members,
    }))
    .expect("encode release manifest");
    fs::write(root.join("manifest.json"), &manifest).expect("write release manifest");
    fs::set_permissions(
        root.join("manifest.json"),
        fs::Permissions::from_mode(0o644),
    )
    .expect("set release manifest mode");
    let candidate = File::open(root.join("bin/hypercolor")).expect("open candidate executable");
    (candidate, manifest)
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("write digest");
    }
    output
}

fn new_store() -> (tempfile::TempDir, InstallStore) {
    let parent = tempfile::tempdir().expect("install parent");
    let store = InstallStore::new(parent.path().join("store"), 64 * 1024);
    (parent, store)
}

fn assert_no_private_residue(store: &InstallStore) {
    let units = store.root().join("units");
    if !units.exists() {
        return;
    }
    for entry in fs::read_dir(units).expect("enumerate units") {
        let name = entry.expect("read unit entry").file_name();
        let name = name.to_string_lossy();
        assert!(
            !name.starts_with(".hypercolor-stage-"),
            "staging residue {name}"
        );
        assert!(
            !name.starts_with(".hypercolor-recovery-"),
            "recovery residue {name}"
        );
    }
}

fn read_authority_file(directory: &ReadOnlyDirectoryAuthority, path: &[&str]) -> Vec<u8> {
    if let [name] = path {
        let mut opened = directory
            .open_regular_file(Path::new(name))
            .expect("open authority file");
        let mut bytes = Vec::new();
        opened
            .file_mut()
            .read_to_end(&mut bytes)
            .expect("read authority file");
        return bytes;
    }
    let child = directory
        .open_child_directory(Path::new(path[0]))
        .expect("open authority directory");
    read_authority_file(&child, &path[1..])
}

fn authority_entry_mode(directory: &ReadOnlyDirectoryAuthority, path: &[&str]) -> u32 {
    if let [name] = path {
        return directory
            .entry_metadata(Path::new(name))
            .expect("inspect authority entry")
            .expect("authority entry exists")
            .mode();
    }
    let child = directory
        .open_child_directory(Path::new(path[0]))
        .expect("open authority directory");
    authority_entry_mode(&child, &path[1..])
}

fn assert_tree_is_immutable(directory: &ReadOnlyDirectoryAuthority) {
    assert_eq!(
        directory.metadata().expect("tree metadata").mode() & 0o222,
        0
    );
    for name in directory.entries().expect("enumerate immutable tree") {
        let metadata = directory
            .entry_metadata(Path::new(&name))
            .expect("entry metadata")
            .expect("entry exists");
        assert_eq!(metadata.mode() & 0o222, 0, "{} is writable", name.display());
        if metadata.kind() == DirectoryEntryKind::Directory {
            let child = directory
                .open_child_directory(Path::new(&name))
                .expect("open immutable directory");
            assert_tree_is_immutable(&child);
        }
    }
}

fn member_mut<'a>(manifest: &'a mut Value, path: &str) -> &'a mut Value {
    manifest["members"]
        .as_array_mut()
        .expect("members array")
        .iter_mut()
        .find(|member| member["path"] == path)
        .expect("manifest member")
}

#[test]
fn valid_payload_stages_immutable_digest_unit_and_reuses_exact_unit() {
    let fixture = ReleaseFixture::new();
    let (_install_parent, store) = new_store();
    let lock = store.acquire_lock().expect("install lock");

    let first = stage_release_payload(&store, &lock, fixture.path(), &fixture.candidate)
        .expect("stage release");
    assert_eq!(first.id().as_str(), sha256(&fixture.manifest));
    assert_eq!(
        read_authority_file(first.directory(), &["manifest.json"]),
        fixture.manifest
    );
    assert_eq!(
        authority_entry_mode(first.directory(), &["manifest.json"]) & 0o7777,
        0o444
    );
    assert_eq!(
        authority_entry_mode(first.directory(), &["bin", "hypercolor"]) & 0o7777,
        0o555
    );
    assert_eq!(
        authority_entry_mode(
            first.directory(),
            &["share", "hypercolor", "ui", "index.html"],
        ) & 0o7777,
        0o444
    );
    assert_eq!(
        first.directory().metadata().expect("unit metadata").mode() & 0o7777,
        0o555
    );
    assert_tree_is_immutable(first.directory());
    let inode = first.directory().metadata().expect("unit metadata").inode();

    let second = stage_release_payload(&store, &lock, fixture.path(), &fixture.candidate)
        .expect("reuse exact release");
    assert_eq!(second, first);
    assert_eq!(
        second
            .directory()
            .metadata()
            .expect("reused metadata")
            .inode(),
        inode
    );
    assert_no_private_residue(&store);
}

#[test]
fn current_executable_mismatch_fails_before_install_state_mutation() {
    let fixture = ReleaseFixture::new();
    let (install_parent, store) = new_store();
    fs::create_dir_all(store.root()).expect("create store");
    fs::write(store.root().join("install.lock"), b"lock-sentinel").expect("seed lock");
    fs::write(
        store.root().join("install-journal.json"),
        b"journal-sentinel",
    )
    .expect("seed journal");
    symlink(
        "units/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        store.root().join("active"),
    )
    .expect("seed active link");
    fs::write(install_parent.path().join("adjacent"), b"outside-sentinel")
        .expect("seed adjacent sentinel");
    let lock = store.acquire_lock().expect("install lock");
    let running = File::open(std::env::current_exe().expect("current executable"))
        .expect("open current executable");

    let error = stage_release_payload(&store, &lock, fixture.path(), &running)
        .expect_err("mismatched running executable must fail");
    assert!(matches!(error, ReleasePayloadError::CandidateMismatch));
    assert_eq!(
        fs::read(store.root().join("install.lock")).expect("read lock"),
        b"lock-sentinel"
    );
    assert_eq!(
        fs::read(store.root().join("install-journal.json")).expect("read journal"),
        b"journal-sentinel"
    );
    assert_eq!(
        fs::read_link(store.root().join("active")).expect("read active"),
        PathBuf::from("units/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
    assert_eq!(
        fs::read(install_parent.path().join("adjacent")).expect("read adjacent"),
        b"outside-sentinel"
    );
    assert!(!store.root().join("units").exists());
}

#[test]
fn strict_manifest_rejects_unknown_duplicate_traversal_and_oversized_inputs() {
    for case in ["unknown", "duplicate", "traversal"] {
        let fixture = ReleaseFixture::new();
        let mut manifest = fixture.manifest_value();
        match case {
            "unknown" => manifest["unknown"] = json!(true),
            "duplicate" => {
                let duplicate = manifest["members"][0].clone();
                manifest["members"]
                    .as_array_mut()
                    .expect("members array")
                    .push(duplicate);
            }
            "traversal" => member_mut(&mut manifest, "bin/hypercolor")["path"] = json!("../escape"),
            _ => unreachable!("known case"),
        }
        fixture.write_manifest(&manifest);
        let (_parent, store) = new_store();
        let lock = store.acquire_lock().expect("install lock");
        stage_release_payload(&store, &lock, fixture.path(), &fixture.candidate)
            .expect_err("malformed manifest must fail");
        assert!(!store.root().join("units").exists());
    }

    let fixture = ReleaseFixture::new();
    fs::write(
        fixture.path().join("manifest.json"),
        vec![b' '; MAX_RELEASE_MANIFEST_BYTES + 1],
    )
    .expect("write oversized manifest");
    let (_parent, store) = new_store();
    let lock = store.acquire_lock().expect("install lock");
    let error = stage_release_payload(&store, &lock, fixture.path(), &fixture.candidate)
        .expect_err("oversized manifest must fail");
    assert!(matches!(
        error,
        ReleasePayloadError::ManifestTooLarge { .. }
    ));
    assert!(!store.root().join("units").exists());
}

#[test]
fn exact_inventory_rejects_missing_and_unexpected_entries() {
    for missing in [false, true] {
        let fixture = ReleaseFixture::new();
        if missing {
            fs::remove_file(fixture.path().join("share/hypercolor/ui/index.html"))
                .expect("remove expected entry");
        } else {
            fs::write(fixture.path().join("unexpected"), b"extra").expect("write extra entry");
        }
        let (_parent, store) = new_store();
        let lock = store.acquire_lock().expect("install lock");
        let error = stage_release_payload(&store, &lock, fixture.path(), &fixture.candidate)
            .expect_err("inventory drift must fail");
        assert!(matches!(error, ReleasePayloadError::InvalidSource(_)));
        assert!(!store.root().join("units").exists());
    }
}

#[test]
fn source_mode_digest_size_and_type_drift_are_rejected() {
    for case in ["mode", "digest", "size", "type"] {
        let fixture = ReleaseFixture::new();
        let asset = fixture.path().join("share/hypercolor/ui/index.html");
        match case {
            "mode" => fs::set_permissions(&asset, fs::Permissions::from_mode(0o600))
                .expect("change source mode"),
            "digest" | "size" | "type" => {
                let mut manifest = fixture.manifest_value();
                let member = member_mut(&mut manifest, "share/hypercolor/ui/index.html");
                match case {
                    "digest" => member["sha256"] = json!("0".repeat(64)),
                    "size" => member["size"] = json!(99),
                    "type" => {
                        member["type"] = json!("directory");
                        member
                            .as_object_mut()
                            .expect("member object")
                            .remove("size");
                        member
                            .as_object_mut()
                            .expect("member object")
                            .remove("sha256");
                    }
                    _ => unreachable!("known case"),
                }
                fixture.write_manifest(&manifest);
            }
            _ => unreachable!("known case"),
        }
        let (_parent, store) = new_store();
        let lock = store.acquire_lock().expect("install lock");
        stage_release_payload(&store, &lock, fixture.path(), &fixture.candidate)
            .expect_err("source metadata drift must fail");
        assert!(!store.root().join("units").exists());
    }
}

#[test]
fn manifest_rejects_unreadable_files_untraversable_directories_and_uppercase_digests() {
    for case in [
        "file-mode",
        "directory-mode",
        "uppercase-digest",
        "asset-count",
    ] {
        let fixture = ReleaseFixture::new();
        let mut manifest = fixture.manifest_value();
        match case {
            "file-mode" => {
                member_mut(&mut manifest, "share/hypercolor/ui/index.html")["mode"] = json!(0o200);
            }
            "directory-mode" => {
                member_mut(&mut manifest, "share/hypercolor/ui")["mode"] = json!(0o400);
            }
            "uppercase-digest" => {
                member_mut(&mut manifest, "share/hypercolor/ui/index.html")["sha256"] =
                    json!(sha256(b"ui").to_uppercase());
            }
            "asset-count" => manifest["assets"]["ui_files"] = json!(2),
            _ => unreachable!("known case"),
        }
        fixture.write_manifest(&manifest);
        let (_parent, store) = new_store();
        let lock = store.acquire_lock().expect("install lock");
        let error = stage_release_payload(&store, &lock, fixture.path(), &fixture.candidate)
            .expect_err("unsafe manifest metadata must fail");
        assert!(
            matches!(
                error,
                ReleasePayloadError::InvalidManifest(_) | ReleasePayloadError::DecodeManifest(_)
            ),
            "unexpected error for {case}: {error}"
        );
        assert!(!store.root().join("units").exists());
    }
}

#[test]
fn source_symlinks_hardlinks_and_special_files_are_rejected() {
    for case in ["symlink", "hardlink", "special"] {
        let fixture = ReleaseFixture::new();
        let asset = fixture.path().join("share/hypercolor/ui/index.html");
        fs::remove_file(&asset).expect("remove asset entry");
        let _listener = match case {
            "symlink" => {
                fs::write(fixture.path().join("outside"), b"ui").expect("write symlink target");
                symlink(fixture.path().join("outside"), &asset).expect("replace with symlink");
                None
            }
            "hardlink" => {
                fs::write(fixture.path().join("outside"), b"ui").expect("write hardlink source");
                fs::hard_link(fixture.path().join("outside"), &asset)
                    .expect("replace with hardlink");
                fs::set_permissions(&asset, fs::Permissions::from_mode(0o644))
                    .expect("set hardlink mode");
                None
            }
            "special" => Some(UnixListener::bind(&asset).expect("replace with socket")),
            _ => unreachable!("known case"),
        };
        let (_parent, store) = new_store();
        let lock = store.acquire_lock().expect("install lock");
        stage_release_payload(&store, &lock, fixture.path(), &fixture.candidate)
            .expect_err("unsafe source member must fail");
        assert!(!store.root().join("units").exists());
    }
}

#[test]
fn retained_source_authority_ignores_parent_and_name_replacement() {
    let parent = tempfile::tempdir().expect("source parent");
    let source = parent.path().join("source");
    let displaced = parent.path().join("displaced");
    fs::create_dir(&source).expect("create source root");
    let (candidate, manifest) = write_release(&source);
    let authority = ReadOnlyDirectoryAuthority::open(&source).expect("source authority");
    fs::rename(&source, &displaced).expect("displace source pathname");
    fs::create_dir(&source).expect("replace source pathname");
    fs::write(source.join("manifest.json"), b"attacker").expect("write attacker manifest");
    let (_install_parent, store) = new_store();
    let lock = store.acquire_lock().expect("install lock");

    let record = stage_release_payload_from_authority(&store, &lock, &authority, &candidate)
        .expect("stage through retained source authority");
    assert_eq!(record.id().as_str(), sha256(&manifest));
    assert_eq!(
        read_authority_file(record.directory(), &["bin", "hypercolor"]),
        b"candidate"
    );
}

#[test]
fn returned_unit_authority_survives_install_root_path_replacement() {
    let fixture = ReleaseFixture::new();
    let source = ReadOnlyDirectoryAuthority::open(fixture.path()).expect("source authority");
    let (install_parent, store) = new_store();
    let lock = store.acquire_lock().expect("install lock");
    let displaced_root = install_parent.path().join("displaced-store");
    fs::rename(store.root(), &displaced_root).expect("displace locked install root");
    fs::create_dir(store.root()).expect("replace install root pathname");
    fs::write(store.root().join("attacker-sentinel"), b"attacker").expect("seed replacement root");

    let record = stage_release_payload_from_authority(&store, &lock, &source, &fixture.candidate)
        .expect("stage beneath retained install root");

    assert_eq!(
        read_authority_file(record.directory(), &["bin", "hypercolor"]),
        b"candidate"
    );
    assert!(
        displaced_root
            .join("units")
            .join(record.id().as_str())
            .is_dir()
    );
    assert!(!store.unit_path(record.id()).exists());
    assert_eq!(
        fs::read(store.root().join("attacker-sentinel")).expect("read replacement sentinel"),
        b"attacker"
    );
    assert!(!store.root().join("units").exists());
}

#[test]
fn source_mutation_after_initial_validation_cleans_private_staging() {
    let fixture = ReleaseFixture::new();
    let large_path = fixture
        .path()
        .join("share/hypercolor/agents/agents/agent.md");
    let large_contents = vec![b'a'; 16 * 1024 * 1024];
    fs::write(&large_path, &large_contents).expect("write large source member");
    let mut manifest = fixture.manifest_value();
    let large_member = member_mut(&mut manifest, "share/hypercolor/agents/agents/agent.md");
    large_member["size"] = json!(large_contents.len());
    large_member["sha256"] = json!(sha256(&large_contents));
    fixture.write_manifest(&manifest);
    let (_install_parent, store) = new_store();
    let lock = store.acquire_lock().expect("install lock");
    let units = store.root().join("units");
    let staged_suffix = Path::new("share/hypercolor/agents/agents/agent.md");

    let error = std::thread::scope(|scope| {
        let mutator = scope.spawn(|| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if units.exists()
                    && fs::read_dir(&units).is_ok_and(|entries| {
                        entries.filter_map(Result::ok).any(|entry| {
                            entry
                                .file_name()
                                .to_string_lossy()
                                .starts_with(".hypercolor-stage-")
                                && entry.path().join(staged_suffix).exists()
                        })
                    })
                {
                    fs::write(&large_path, vec![b'b'; large_contents.len()])
                        .expect("mutate source during staging");
                    return;
                }
                assert!(Instant::now() < deadline, "staging never became visible");
                std::thread::yield_now();
            }
        });
        let result = stage_release_payload(&store, &lock, fixture.path(), &fixture.candidate)
            .expect_err("concurrent source mutation must fail");
        mutator.join().expect("source mutator");
        result
    });

    assert!(
        matches!(
            error,
            ReleasePayloadError::InvalidSource(_)
                | ReleasePayloadError::Filesystem {
                    operation: "copy a release member into private staging",
                    ..
                }
        ),
        "unexpected mutation error: {error}"
    );
    assert_no_private_residue(&store);
    assert!(
        fs::read_dir(store.root().join("units"))
            .expect("enumerate units")
            .next()
            .is_none(),
        "failed mutation must not publish a unit"
    );
}

#[test]
fn corrupt_existing_digest_unit_is_refused_without_replacement() {
    let fixture = ReleaseFixture::new();
    let (_install_parent, store) = new_store();
    let lock = store.acquire_lock().expect("install lock");
    let record = stage_release_payload(&store, &lock, fixture.path(), &fixture.candidate)
        .expect("stage release");
    let unit_root = store.unit_path(record.id());
    let binary = unit_root.join("bin/hypercolor");
    let inode = record
        .directory()
        .metadata()
        .expect("unit metadata")
        .inode();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))
        .expect("corrupt immutable mode");

    let error = stage_release_payload(&store, &lock, fixture.path(), &fixture.candidate)
        .expect_err("corrupt existing digest unit must fail");
    assert!(matches!(error, ReleasePayloadError::InvalidUnit(_)));
    assert_eq!(
        record
            .directory()
            .metadata()
            .expect("unit metadata")
            .inode(),
        inode
    );
    assert_eq!(
        fs::metadata(&binary).expect("binary metadata").mode() & 0o7777,
        0o755
    );
    assert_no_private_residue(&store);
}
