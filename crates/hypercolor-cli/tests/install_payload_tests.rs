#![cfg(unix)]

use std::fs::{self, File};
use std::io::Read as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use hypercolor_cli::install::bind_macos_release_provenance;
use hypercolor_cli::install::{
    InstallLock, InstallStore, MAX_RELEASE_MANIFEST_BYTES, ReleasePayloadError, UnitId, UnitRecord,
    retain_linux_unit, stage_release_payload, stage_release_payload_from_authority,
    validate_release_payload,
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

    fn expected_unit(&self) -> UnitId {
        let manifest = fs::read(self.path().join("manifest.json")).expect("read manifest");
        UnitId::new(sha256(&manifest)).expect("valid manifest digest")
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
        "share/hypercolor/launchd",
    ];
    let designated_requirement = concat!(
        "designated => identifier \"tech.hyperbliss.hypercolor.daemon\" and ",
        "anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] ",
        "/* exists */ and certificate leaf[field.1.2.840.113635.100.6.1.13] ",
        "/* exists */ and certificate leaf[subject.OU] = \"AB12CD34EF\""
    );
    let (daemon_bytes, daemon_cdhash) = thin_macho_daemon();
    let (_, rust_target) = native_macos_identity();
    let provenance = serde_json::to_vec_pretty(&json!({
        "team_id": "AB12CD34EF",
        "target": rust_target,
        "objects": [{
            "path": "bin/hypercolor-daemon",
            "identifier": "tech.hyperbliss.hypercolor.daemon",
            "designated_requirement": designated_requirement,
            "cdhash": daemon_cdhash,
        }],
        "notarization": {
            "id": "2efe2717-52ef-43a5-96dc-0797e4ca1041",
            "message": "Processing complete",
            "status": "Accepted",
        },
    }))
    .expect("encode macOS provenance");
    let files = vec![
        ("bin/hypercolor-daemon", daemon_bytes),
        ("bin/hypercolor", b"candidate".to_vec()),
        ("bin/hypercolor-app", b"app".to_vec()),
        ("bin/hypercolor-tui", b"tui".to_vec()),
        ("bin/hypercolor-open", b"open".to_vec()),
        ("share/hypercolor/ui/index.html", b"ui".to_vec()),
        (
            "share/hypercolor/effects/bundled/effect.html",
            b"effect".to_vec(),
        ),
        ("share/hypercolor/agents/skills/skill.md", b"skill".to_vec()),
        ("share/hypercolor/agents/agents/agent.md", b"agent".to_vec()),
        (
            "share/hypercolor/launchd/tech.hyperbliss.hypercolor.plist",
            b"plist".to_vec(),
        ),
        ("share/hypercolor/macos-notarization.json", provenance),
    ];
    let mut members = Vec::new();
    for directory in directories {
        fs::create_dir_all(root.join(directory)).expect("create release directory");
        fs::set_permissions(root.join(directory), fs::Permissions::from_mode(0o755))
            .expect("set release directory mode");
        members.push(json!({"path": directory, "type": "directory", "mode": 0o755}));
    }
    for (path, bytes) in files {
        fs::write(root.join(path), &bytes).expect("write release file");
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
            "sha256": sha256(&bytes),
        }));
    }
    members.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let (platform, rust_target) = native_macos_identity();
    let manifest = serde_json::to_vec_pretty(&json!({
        "name": "hypercolor",
        "version": "0.3.2",
        "platform": platform,
        "rust_target": rust_target,
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

fn native_macos_identity() -> (&'static str, &'static str) {
    match std::env::consts::ARCH {
        "aarch64" => ("macos-arm64", "aarch64-apple-darwin"),
        "x86_64" => ("macos-amd64", "x86_64-apple-darwin"),
        architecture => panic!("unsupported test architecture {architecture}"),
    }
}

fn thin_macho_daemon() -> (Vec<u8>, String) {
    const MACH_MAGIC_64: u32 = 0xfeed_facf;
    const LC_CODE_SIGNATURE: u32 = 0x1d;
    const EMBEDDED_SIGNATURE: u32 = 0xfade_0cc0;
    const CODE_DIRECTORY: u32 = 0xfade_0c02;
    let cpu_type = match std::env::consts::ARCH {
        "aarch64" => 0x0100_000c_u32,
        "x86_64" => 0x0100_0007_u32,
        architecture => panic!("unsupported fixture architecture {architecture}"),
    };
    let mut code_directory = vec![0_u8; 40];
    code_directory[0..4].copy_from_slice(&CODE_DIRECTORY.to_be_bytes());
    code_directory[4..8].copy_from_slice(&40_u32.to_be_bytes());
    code_directory[36] = 32;
    code_directory[37] = 2;
    let mut signature = vec![0_u8; 20];
    signature[0..4].copy_from_slice(&EMBEDDED_SIGNATURE.to_be_bytes());
    signature[4..8].copy_from_slice(&60_u32.to_be_bytes());
    signature[8..12].copy_from_slice(&1_u32.to_be_bytes());
    signature[12..16].copy_from_slice(&0_u32.to_be_bytes());
    signature[16..20].copy_from_slice(&20_u32.to_be_bytes());
    signature.extend_from_slice(&code_directory);
    let mut macho = vec![0_u8; 48];
    macho[0..4].copy_from_slice(&MACH_MAGIC_64.to_le_bytes());
    macho[4..8].copy_from_slice(&cpu_type.to_le_bytes());
    macho[16..20].copy_from_slice(&1_u32.to_le_bytes());
    macho[20..24].copy_from_slice(&16_u32.to_le_bytes());
    macho[32..36].copy_from_slice(&LC_CODE_SIGNATURE.to_le_bytes());
    macho[36..40].copy_from_slice(&16_u32.to_le_bytes());
    macho[40..44].copy_from_slice(&48_u32.to_le_bytes());
    macho[44..48].copy_from_slice(&60_u32.to_le_bytes());
    macho.extend_from_slice(&signature);
    let digest = Sha256::digest(code_directory);
    (macho, hex_bytes(&digest[..20]))
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

fn hex_bytes(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("write hexadecimal bytes");
    }
    output
}

fn new_store() -> (tempfile::TempDir, InstallStore) {
    let parent = tempfile::tempdir().expect("install parent");
    let store = InstallStore::new(parent.path().join("store"), 64 * 1024);
    (parent, store)
}

fn stage_fixture(
    store: &InstallStore,
    lock: &InstallLock,
    fixture: &ReleaseFixture,
) -> Result<UnitRecord, ReleasePayloadError> {
    stage_fixture_with_candidate(store, lock, fixture, &fixture.candidate)
}

fn stage_fixture_with_candidate(
    store: &InstallStore,
    lock: &InstallLock,
    fixture: &ReleaseFixture,
    candidate: &File,
) -> Result<UnitRecord, ReleasePayloadError> {
    stage_release_payload(
        store,
        lock,
        fixture.path(),
        candidate,
        &fixture.expected_unit(),
    )
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

#[cfg(target_os = "macos")]
fn rewrite_member(fixture: &ReleaseFixture, path: &str, bytes: &[u8]) {
    fs::write(fixture.path().join(path), bytes).expect("rewrite release member");
    let mut manifest = fixture.manifest_value();
    let member = member_mut(&mut manifest, path);
    member["size"] = json!(bytes.len());
    member["sha256"] = json!(sha256(bytes));
    fixture.write_manifest(&manifest);
}

#[test]
fn valid_payload_stages_immutable_digest_unit_and_reuses_exact_unit() {
    let fixture = ReleaseFixture::new();
    let (_install_parent, store) = new_store();
    let lock = store.acquire_lock().expect("install lock");

    let first = stage_fixture(&store, &lock, &fixture).expect("stage release");
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

    let second = stage_fixture(&store, &lock, &fixture).expect("reuse exact release");
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

#[cfg(target_os = "macos")]
#[test]
fn retained_macos_unit_binds_manifest_and_notarized_daemon_identity() {
    let fixture = ReleaseFixture::new();
    let (_install_parent, store) = new_store();
    let lock = store.acquire_lock().expect("install lock");
    let unit = stage_fixture(&store, &lock, &fixture).expect("stage release");

    let provenance = bind_macos_release_provenance(&unit).expect("bind macOS provenance");
    let daemon = unit
        .directory()
        .open_child_directory(Path::new("bin"))
        .expect("open retained bin directory")
        .open_regular_file(Path::new("hypercolor-daemon"))
        .expect("open retained daemon");
    let (daemon_bytes, daemon_cdhash) = thin_macho_daemon();

    assert_eq!(provenance.daemon_sha256(), sha256(&daemon_bytes));
    assert_eq!(provenance.daemon_size(), daemon_bytes.len() as u64);
    assert_eq!(provenance.daemon_mode(), 0o555);
    assert_eq!(provenance.daemon_device(), daemon.metadata().device());
    assert_eq!(provenance.daemon_inode(), daemon.metadata().inode());
    assert_eq!(provenance.team_id(), "AB12CD34EF");
    assert_eq!(provenance.cdhash(), daemon_cdhash);
    assert_eq!(
        provenance.designated_requirement(),
        concat!(
            "identifier \"tech.hyperbliss.hypercolor.daemon\" and ",
            "anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] ",
            "/* exists */ and certificate leaf[field.1.2.840.113635.100.6.1.13] ",
            "/* exists */ and certificate leaf[subject.OU] = \"AB12CD34EF\""
        )
    );
    assert_eq!(
        provenance.designated_requirement_sha256(),
        sha256(provenance.designated_requirement().as_bytes())
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_provenance_rejects_malformed_or_mismatched_identity() {
    for case in [
        "platform",
        "daemon-identifier",
        "designated-requirement",
        "designated-requirement-broadened",
        "cdhash-uppercase",
        "cdhash-mismatch",
        "notarization-field",
        "notarization-message-bound",
        "object-count",
    ] {
        let fixture = ReleaseFixture::new();
        match case {
            "platform" => {
                let mut manifest = fixture.manifest_value();
                manifest["platform"] = json!("linux-x86_64");
                fixture.write_manifest(&manifest);
            }
            "daemon-identifier"
            | "designated-requirement"
            | "designated-requirement-broadened"
            | "cdhash-uppercase"
            | "cdhash-mismatch"
            | "notarization-field"
            | "notarization-message-bound"
            | "object-count" => {
                let path = "share/hypercolor/macos-notarization.json";
                let mut provenance: Value = serde_json::from_slice(
                    &fs::read(fixture.path().join(path)).expect("read provenance"),
                )
                .expect("decode provenance");
                match case {
                    "daemon-identifier" => {
                        provenance["objects"][0]["identifier"] =
                            json!("tech.hyperbliss.hypercolor.attacker");
                    }
                    "designated-requirement" => {
                        provenance["objects"][0]["designated_requirement"] = json!("bogus");
                    }
                    "designated-requirement-broadened" => {
                        let requirement = provenance["objects"][0]["designated_requirement"]
                            .as_str()
                            .expect("fixture designated requirement");
                        provenance["objects"][0]["designated_requirement"] =
                            json!(format!("{requirement} or anchor trusted"));
                    }
                    "cdhash-uppercase" => {
                        let cdhash = provenance["objects"][0]["cdhash"]
                            .as_str()
                            .expect("fixture CDHash");
                        provenance["objects"][0]["cdhash"] = json!(cdhash.to_ascii_uppercase());
                    }
                    "cdhash-mismatch" => {
                        provenance["objects"][0]["cdhash"] = json!("0".repeat(40));
                    }
                    "notarization-field" => {
                        provenance["notarization"]["attacker"] = json!(true);
                    }
                    "notarization-message-bound" => {
                        provenance["notarization"]["message"] = json!("x".repeat(900 * 1024));
                    }
                    "object-count" => {
                        let object = provenance["objects"][0].clone();
                        provenance["objects"] =
                            Value::Array(std::iter::repeat_n(object, 129).collect());
                    }
                    _ => unreachable!("known provenance case"),
                }
                rewrite_member(
                    &fixture,
                    path,
                    &serde_json::to_vec_pretty(&provenance).expect("encode provenance"),
                );
            }
            _ => unreachable!("known case"),
        }
        let (_install_parent, store) = new_store();
        let lock = store.acquire_lock().expect("install lock");
        let unit = stage_fixture(&store, &lock, &fixture).expect("stage release");

        let error = bind_macos_release_provenance(&unit)
            .expect_err("mismatched macOS identity must fail closed");

        assert!(matches!(error, ReleasePayloadError::InvalidUnit(_)));
    }
}

#[test]
fn verified_manifest_digest_mismatch_fails_before_install_state_mutation() {
    let fixture = ReleaseFixture::new();
    let (_install_parent, store) = new_store();
    fs::create_dir_all(store.root()).expect("create store");
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
    let lock = store.acquire_lock().expect("install lock");
    let expected = UnitId::new("0".repeat(64)).expect("valid expected digest");
    let actual = fixture.expected_unit();

    let error = stage_release_payload(&store, &lock, fixture.path(), &fixture.candidate, &expected)
        .expect_err("unverified manifest digest must fail");

    assert!(matches!(
        error,
        ReleasePayloadError::UnexpectedManifestDigest {
            expected: ref reported_expected,
            actual: ref reported_actual,
        } if reported_expected == expected.as_str() && reported_actual == actual.as_str()
    ));
    assert_eq!(
        fs::read(store.root().join("install-journal.json")).expect("read journal"),
        b"journal-sentinel"
    );
    assert_eq!(
        fs::read_link(store.root().join("active")).expect("read active"),
        PathBuf::from("units/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
    assert!(!store.root().join("units").exists());
    assert_no_private_residue(&store);
}

#[test]
fn release_preflight_validates_before_store_bootstrap() {
    let fixture = ReleaseFixture::new();
    let (install_parent, store) = new_store();
    let expected = fixture.expected_unit();

    validate_release_payload(fixture.path(), &fixture.candidate, &expected)
        .expect("validate release before install authority exists");
    assert!(!store.root().exists());

    let mismatch = UnitId::new("0".repeat(64)).expect("valid mismatched digest");
    let error = validate_release_payload(fixture.path(), &fixture.candidate, &mismatch)
        .expect_err("mismatched release must fail before store bootstrap");
    assert!(matches!(
        error,
        ReleasePayloadError::UnexpectedManifestDigest { .. }
    ));
    assert!(!store.root().exists());
    assert!(
        fs::read_dir(install_parent.path())
            .expect("inspect install parent")
            .next()
            .is_none()
    );
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

    let error = stage_fixture_with_candidate(&store, &lock, &fixture, &running)
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
        stage_fixture(&store, &lock, &fixture).expect_err("malformed manifest must fail");
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
    let error = stage_fixture(&store, &lock, &fixture).expect_err("oversized manifest must fail");
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
        let error = stage_fixture(&store, &lock, &fixture).expect_err("inventory drift must fail");
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
        stage_fixture(&store, &lock, &fixture).expect_err("source metadata drift must fail");
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
        let error =
            stage_fixture(&store, &lock, &fixture).expect_err("unsafe manifest metadata must fail");
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
        stage_fixture(&store, &lock, &fixture).expect_err("unsafe source member must fail");
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

    let expected_unit = UnitId::new(sha256(&manifest)).expect("valid manifest digest");
    let record =
        stage_release_payload_from_authority(&store, &lock, &authority, &candidate, &expected_unit)
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

    let record = stage_release_payload_from_authority(
        &store,
        &lock,
        &source,
        &fixture.candidate,
        &fixture.expected_unit(),
    )
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
fn cold_rebind_validates_the_exact_installed_release() {
    let fixture = ReleaseFixture::new();
    let (_install_parent, store) = new_store();
    let lock = store.acquire_lock().expect("install lock");
    let staged = stage_fixture(&store, &lock, &fixture).expect("stage release");
    let unit = staged.id().clone();
    drop(staged);

    let rebound = retain_linux_unit(&store, &lock, &unit).expect("rebind installed release");

    assert_eq!(rebound.id(), &unit);
    assert_eq!(
        read_authority_file(rebound.directory(), &["bin", "hypercolor"]),
        b"candidate"
    );
}

#[test]
fn cold_rebind_rejects_wrong_lock_missing_unit_and_manifest_id_mismatch() {
    let fixture = ReleaseFixture::new();
    let (_install_parent, store) = new_store();
    let lock = store.acquire_lock().expect("install lock");
    let staged = stage_fixture(&store, &lock, &fixture).expect("stage release");
    let unit = staged.id().clone();
    drop(staged);
    let (_foreign_parent, foreign_store) = new_store();
    let foreign_lock = foreign_store.acquire_lock().expect("foreign install lock");

    let wrong_lock = retain_linux_unit(&store, &foreign_lock, &unit)
        .expect_err("foreign lock must not retain a unit");
    assert!(wrong_lock.to_string().contains("another prefix"));

    let missing = UnitId::new("a".repeat(64)).expect("valid missing unit id");
    let missing_error =
        retain_linux_unit(&store, &lock, &missing).expect_err("missing unit must not be retained");
    assert!(
        missing_error
            .to_string()
            .contains("open the exact immutable unit")
    );

    fs::rename(store.unit_path(&unit), store.unit_path(&missing))
        .expect("rename unit beneath retained store");
    let mismatch = retain_linux_unit(&store, &lock, &missing)
        .expect_err("manifest identity mismatch must fail");
    assert!(
        mismatch
            .to_string()
            .contains("does not match verified digest")
    );
}

#[test]
fn cold_rebind_rejects_installed_content_drift() {
    let fixture = ReleaseFixture::new();
    let (_install_parent, store) = new_store();
    let lock = store.acquire_lock().expect("install lock");
    let staged = stage_fixture(&store, &lock, &fixture).expect("stage release");
    let unit = staged.id().clone();
    drop(staged);
    let binary = store.unit_path(&unit).join("bin/hypercolor");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))
        .expect("make installed binary writable for adversarial mutation");
    fs::write(&binary, b"corrupt!!").expect("replace installed bytes");

    let error = retain_linux_unit(&store, &lock, &unit)
        .expect_err("content drift must prevent cold rebind");

    assert!(error.to_string().contains("invalid immutable release unit"));
}

#[test]
fn cold_rebind_uses_retained_root_after_path_replacement() {
    let fixture = ReleaseFixture::new();
    let (install_parent, store) = new_store();
    let lock = store.acquire_lock().expect("install lock");
    let staged = stage_fixture(&store, &lock, &fixture).expect("stage release");
    let unit = staged.id().clone();
    drop(staged);
    let displaced_root = install_parent.path().join("cold-rebind-store");
    fs::rename(store.root(), &displaced_root).expect("displace locked install root");
    fs::create_dir(store.root()).expect("replace install root pathname");
    fs::write(store.root().join("attacker-sentinel"), b"attacker").expect("seed replacement root");

    let rebound =
        retain_linux_unit(&store, &lock, &unit).expect("rebind through retained install authority");

    assert_eq!(
        read_authority_file(rebound.directory(), &["bin", "hypercolor"]),
        b"candidate"
    );
    assert!(!store.root().join("units").exists());
    assert!(displaced_root.join("units").join(unit.as_str()).is_dir());
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
        let result = stage_fixture(&store, &lock, &fixture)
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
    let record = stage_fixture(&store, &lock, &fixture).expect("stage release");
    let unit_root = store.unit_path(record.id());
    let binary = unit_root.join("bin/hypercolor");
    let inode = record
        .directory()
        .metadata()
        .expect("unit metadata")
        .inode();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))
        .expect("corrupt immutable mode");

    let error =
        stage_fixture(&store, &lock, &fixture).expect_err("corrupt existing digest unit must fail");
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
