use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::{
    LAYOUT, Source, VENDOR_SIGNED, WINDOWS_DURABLE_STORES_PATH, WindowsUnitError,
    WindowsUnitSources, is_pe_image, platform_of, produce_windows_unit,
};
use crate::windows::manifest::{Member, ValidatedManifest};
use crate::windows::{PermissionClass, WindowsVendor};

/// Bytes Windows would load as an image: a DOS header pointing at a PE
/// signature, then `tag` so each program hashes differently.
fn pe(tag: &str) -> Vec<u8> {
    let mut bytes = vec![0_u8; 0x80];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[0x3C..0x40].copy_from_slice(&0x40_u32.to_le_bytes());
    bytes[0x40..0x44].copy_from_slice(b"PE\0\0");
    bytes.extend_from_slice(tag.as_bytes());
    bytes
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("parent directory");
    fs::write(path, bytes).expect("write fixture");
}

/// The engine's own inventory, so the unit is checked against the stores a
/// real release declares.
fn engine_inventory() -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/managed/durable-stores.json"),
    )
    .expect("engine store inventory")
}

struct Fixture {
    _guard: tempfile::TempDir,
    base: PathBuf,
    sources: WindowsUnitSources,
}

impl Fixture {
    fn new() -> Self {
        let guard = tempfile::tempdir().expect("temporary directory");
        let base = fs::canonicalize(guard.path()).expect("canonical base");
        let base = PathBuf::from(
            base.to_str()
                .expect("UTF-8 temporary path")
                .trim_start_matches(r"\\?\"),
        );
        let bin = base.join("bin");
        let stage = base.join("stage");
        let source = base.join("source");
        for placement in LAYOUT {
            match placement.source {
                Source::Built(name) => write(&bin.join(name), &pe(name)),
                Source::Staged(path) if !placement.tree => write(&stage.join(path), &pe(path)),
                Source::Staged(_) => {}
                Source::Repository(path) => {
                    write(&source.join(path), format!("{path} text\n").as_bytes());
                }
            }
        }
        write(&stage.join("ui/index.html"), b"<html></html>");
        write(&stage.join("ui/assets/app-3f2a.js"), b"console.log(1)");
        write(&stage.join("effects/rainbow.html"), b"rainbow");
        write(
            &stage.join("tools/pawnio/PawnIO_setup.exe"),
            &pe("pawnio setup"),
        );
        write(
            &stage.join("tools/pawnio/PawnIO.Modules.zip"),
            b"PK\x03\x04modules",
        );
        write(&stage.join("tools/pawnio/manifest.json"), b"{}");
        write(&stage.join("tools/pawnio/modules/SmbusI801.bin"), b"module");
        write(
            &source.join("packaging/managed/durable-stores.json"),
            &engine_inventory(),
        );
        let sources = WindowsUnitSources {
            version: "0.9.0".to_owned(),
            rust_target: "x86_64-pc-windows-msvc".to_owned(),
            bin_dir: bin,
            stage_dir: stage,
            source_root: source,
            durable_store_overlays: Vec::new(),
            third_party_notices: None,
        };
        Self {
            _guard: guard,
            base,
            sources,
        }
    }

    fn output(&self, name: &str) -> PathBuf {
        self.base.join(name)
    }

    fn overlay(&self, name: &str, value: &Value) -> PathBuf {
        let path = self.base.join(name);
        write(&path, &serde_json::to_vec(value).expect("overlay"));
        path
    }
}

fn parse(root: &Path) -> ValidatedManifest {
    ValidatedManifest::parse(fs::read(root.join("manifest.json")).expect("manifest"))
        .expect("the produced manifest parses")
}

/// Every entry under `root` except the manifest, as unit paths.
fn tree_entries(root: &Path, prefix: &str, entries: &mut BTreeSet<String>) {
    for entry in fs::read_dir(root).expect("list") {
        let entry = entry.expect("entry");
        let name = entry.file_name().into_string().expect("UTF-8 name");
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if path == "manifest.json" {
            continue;
        }
        if entry.file_type().expect("type").is_dir() {
            tree_entries(&entry.path(), &path, entries);
        }
        entries.insert(path);
    }
}

#[test]
fn a_unit_holds_exactly_its_manifest_with_programs_classed_by_their_bytes() {
    let fixture = Fixture::new();
    let output = fixture.output("unit");
    let unit = produce_windows_unit(&fixture.sources, &output).expect("produce");

    let bytes = fs::read(output.join("manifest.json")).expect("manifest");
    assert_eq!(unit.unit_id().as_str(), hex::encode(Sha256::digest(&bytes)));
    assert_eq!(unit.root(), output);
    let manifest = parse(&output);
    assert_eq!(&manifest.unit_id, unit.unit_id());

    let mut on_disk = BTreeSet::new();
    tree_entries(&output, "", &mut on_disk);
    assert_eq!(
        on_disk,
        manifest.members.keys().cloned().collect::<BTreeSet<_>>(),
        "the tree holds exactly the members its manifest declares"
    );
    for (path, member) in &manifest.members {
        let Member::File {
            class,
            size,
            sha256,
        } = member
        else {
            continue;
        };
        let written = fs::read(output.join(path)).expect("member");
        assert_eq!(written.len() as u64, *size, "{path} size");
        assert_eq!(
            &hex::encode(Sha256::digest(&written)),
            sha256,
            "{path} digest"
        );
        let expected = if written.starts_with(b"MZ") {
            PermissionClass::Program
        } else {
            PermissionClass::Data
        };
        assert_eq!(*class, expected, "{path} class");
    }
    for program in [
        "hypercolor-app.exe",
        "libEGL.dll",
        "tools/hypercolor-smbus-service.exe",
        "tools/pawnio/PawnIO_setup.exe",
    ] {
        assert!(
            matches!(
                manifest.members[program],
                Member::File {
                    class: PermissionClass::Program,
                    ..
                }
            ),
            "{program} is a program"
        );
    }
    for data in [
        "tools/install-windows-hardware-support.ps1",
        "tools/pawnio/PawnIO.Modules.zip",
        "ui/index.html",
        "LICENSE",
        WINDOWS_DURABLE_STORES_PATH,
    ] {
        assert!(
            matches!(
                manifest.members[data],
                Member::File {
                    class: PermissionClass::Data,
                    ..
                }
            ),
            "{data} is data"
        );
    }

    let expected_vendors: Vec<(String, WindowsVendor)> = {
        let mut list: Vec<_> = VENDOR_SIGNED
            .iter()
            .map(|(path, vendor)| ((*path).to_owned(), *vendor))
            .collect();
        list.sort();
        list
    };
    assert_eq!(unit.vendor_signed(), expected_vendors);
    assert_eq!(
        manifest.vendor_signed.into_iter().collect::<Vec<_>>(),
        expected_vendors
    );

    let raw: Value = serde_json::from_slice(&bytes).expect("manifest JSON");
    assert_eq!(raw["platform"], "windows-x86_64");
    assert_eq!(raw["version"], "0.9.0");
    assert!(bytes.ends_with(b"}\n"));
}

#[test]
fn the_same_inputs_produce_the_same_unit() {
    let fixture = Fixture::new();
    let first = produce_windows_unit(&fixture.sources, &fixture.output("first")).expect("first");
    let second = produce_windows_unit(&fixture.sources, &fixture.output("second")).expect("second");
    assert_eq!(first.unit_id(), second.unit_id());

    let mut changed = fixture.sources.clone();
    changed.version = "0.9.1".to_owned();
    let third = produce_windows_unit(&changed, &fixture.output("third")).expect("third");
    assert_ne!(first.unit_id(), third.unit_id());
}

#[test]
fn the_unit_ships_the_engine_inventory_with_each_overlay_appended() {
    let fixture = Fixture::new();
    let mut sources = fixture.sources.clone();
    sources.durable_store_overlays.push(fixture.overlay(
        "official-stores.json",
        &json!({"stores": [{"name": "update-records", "storage_format": "json"}]}),
    ));
    let output = fixture.output("unit");
    produce_windows_unit(&sources, &output).expect("produce");

    let shipped = fs::read(output.join(WINDOWS_DURABLE_STORES_PATH)).expect("inventory");
    let shipped: Value = serde_json::from_slice(&shipped).expect("inventory JSON");
    let mut expected: Value = serde_json::from_slice(&engine_inventory()).expect("engine");
    expected["stores"]
        .as_array_mut()
        .expect("stores")
        .push(json!({"name": "update-records", "storage_format": "json"}));
    assert_eq!(shipped, expected);
    let text = fs::read_to_string(output.join(WINDOWS_DURABLE_STORES_PATH)).expect("text");
    let first_store = text.find("\"migration_mode\"").expect("a sorted key");
    assert!(
        first_store < text.find("\"name\"").expect("name key"),
        "keys are written in order"
    );
}

#[test]
fn overlays_that_redeclare_or_misshape_stores_are_refused() {
    let fixture = Fixture::new();
    let cases = [
        ("again", json!({"stores": [{"name": "config"}]})),
        ("extra-key", json!({"stores": [], "version": 1})),
        ("unnamed", json!({"stores": [{"storage_format": "json"}]})),
        ("not-a-list", json!({"stores": {"name": "x"}})),
    ];
    for (name, overlay) in cases {
        let mut sources = fixture.sources.clone();
        sources
            .durable_store_overlays
            .push(fixture.overlay(&format!("{name}.json"), &overlay));
        let output = fixture.output(name);
        assert!(
            matches!(
                produce_windows_unit(&sources, &output),
                Err(WindowsUnitError::InvalidSources(_))
            ),
            "{name} must be refused"
        );
        assert!(!output.exists(), "{name} leaves nothing behind");
    }
}

#[test]
fn third_party_notices_ship_beside_the_license() {
    let fixture = Fixture::new();
    let notices = fixture.base.join("notices");
    write(&notices.join("libvpx/LICENSE"), b"BSD");
    let mut sources = fixture.sources.clone();
    sources.third_party_notices = Some(notices.clone());
    let output = fixture.output("unit");
    produce_windows_unit(&sources, &output).expect("produce");
    let manifest = parse(&output);
    assert_eq!(
        manifest.members.get("third-party"),
        Some(&Member::Directory)
    );
    assert!(manifest.members.contains_key("third-party/libvpx/LICENSE"));

    let empty = fixture.base.join("empty-notices");
    fs::create_dir_all(empty.join("nothing")).expect("empty notices");
    sources.third_party_notices = Some(empty);
    assert!(produce_windows_unit(&sources, &fixture.output("empty")).is_err());
}

#[test]
fn incomplete_or_unloadable_inputs_are_refused_and_leave_nothing() {
    type Break = fn(&WindowsUnitSources);
    let cases: [(&str, Break); 4] = [
        ("no-daemon", |sources| {
            fs::remove_file(sources.bin_dir.join("hypercolor-daemon.exe")).expect("remove");
        }),
        ("cli-not-an-image", |sources| {
            fs::write(sources.bin_dir.join("hypercolor.exe"), b"#!/bin/sh").expect("replace");
        }),
        ("runtime-not-an-image", |sources| {
            fs::write(sources.stage_dir.join("tools/vcruntime140.dll"), b"text").expect("replace");
        }),
        ("no-pawnio-setup", |sources| {
            fs::remove_file(sources.stage_dir.join("tools/pawnio/PawnIO_setup.exe"))
                .expect("remove");
        }),
    ];
    for (name, break_inputs) in cases {
        let fixture = Fixture::new();
        break_inputs(&fixture.sources);
        let output = fixture.output("unit");
        assert!(
            produce_windows_unit(&fixture.sources, &output).is_err(),
            "{name} must be refused"
        );
        assert!(!output.exists(), "{name} leaves nothing behind");
    }

    let fixture = Fixture::new();
    let taken = fixture.output("taken");
    fs::create_dir(&taken).expect("existing output");
    File::create(taken.join("keep")).expect("existing file");
    assert!(produce_windows_unit(&fixture.sources, &taken).is_err());
    assert!(
        taken.join("keep").exists(),
        "an existing output is never touched"
    );
}

/// Windows itself cannot hold these inputs: it maps `aux.js` to a device
/// and merges names that differ only in case.
#[cfg(unix)]
#[test]
fn links_device_names_and_case_collisions_in_the_inputs_are_refused() {
    let fixture = Fixture::new();
    std::os::unix::fs::symlink("/etc/passwd", fixture.sources.stage_dir.join("ui/passwd"))
        .expect("link");
    assert!(produce_windows_unit(&fixture.sources, &fixture.output("linked")).is_err());

    let fixture = Fixture::new();
    write(&fixture.sources.stage_dir.join("ui/aux.js"), b"x");
    let output = fixture.output("device");
    assert!(produce_windows_unit(&fixture.sources, &output).is_err());
    assert!(!output.exists());

    let fixture = Fixture::new();
    write(&fixture.sources.stage_dir.join("ui/\u{3c3}.js"), b"medial");
    write(&fixture.sources.stage_dir.join("ui/\u{3c2}.js"), b"final");
    assert!(
        produce_windows_unit(&fixture.sources, &fixture.output("sigma")).is_err(),
        "NTFS folds both sigmas to one name"
    );

    let fixture = Fixture::new();
    write(&fixture.sources.stage_dir.join("ui/Index.html"), b"other");
    assert!(
        matches!(
            produce_windows_unit(&fixture.sources, &fixture.output("folded")),
            Err(WindowsUnitError::InvalidSources(message)) if message.contains("case-insensitive")
        ),
        "two names a Windows volume merges are refused"
    );
}

#[test]
fn only_a_dos_header_pointing_at_a_pe_signature_is_an_image() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let check = |bytes: &[u8]| {
        let path = directory.path().join("probe");
        File::create(&path)
            .and_then(|mut file| file.write_all(bytes))
            .expect("probe");
        let mut file = File::open(&path).expect("open probe");
        is_pe_image(&mut file, bytes.len() as u64).expect("read probe")
    };
    assert!(check(&pe("")));

    let mut far = vec![0_u8; 0x204];
    far[..2].copy_from_slice(b"MZ");
    far[0x3C..0x40].copy_from_slice(&0x200_u32.to_le_bytes());
    far[0x200..].copy_from_slice(b"PE\0\0");
    assert!(check(&far), "the signature may sit anywhere in the file");

    let mut past_end = pe("");
    past_end[0x3C..0x40].copy_from_slice(&0x1000_u32.to_le_bytes());
    assert!(!check(&past_end));
    let mut wrong_signature = pe("");
    wrong_signature[0x40..0x44].copy_from_slice(b"NE\0\0");
    assert!(!check(&wrong_signature));
    assert!(!check(b"MZ"), "too short to hold a DOS header");
    assert!(!check(&[0_u8; 0x80]));
}

#[test]
fn only_msvc_windows_targets_name_a_platform() {
    assert_eq!(
        platform_of("x86_64-pc-windows-msvc").expect("x86_64"),
        "windows-x86_64"
    );
    assert_eq!(
        platform_of("aarch64-pc-windows-msvc").expect("aarch64"),
        "windows-aarch64"
    );
    for target in [
        "x86_64-pc-windows-gnu",
        "x86_64-unknown-linux-gnu",
        "-pc-windows-msvc",
        "x86/64-pc-windows-msvc",
    ] {
        assert!(platform_of(target).is_err(), "{target} must be refused");
    }
}

/// Until the installer installs units, it lays out the same tree from the
/// Tauri bundle configuration, so the unit layout must name exactly the
/// paths those files install.
#[test]
fn the_unit_layout_is_the_tree_the_installer_lays_out() {
    let app = Path::new(env!("CARGO_MANIFEST_DIR")).join("../hypercolor-app");
    let mut installed = BTreeSet::from(["hypercolor-app.exe".to_owned()]);
    for config in [
        "tauri.conf.json",
        "tauri.bundle.conf.json",
        "tauri.windows.bundle.conf.json",
    ] {
        let value: Value =
            serde_json::from_slice(&fs::read(app.join(config)).expect("Tauri config"))
                .expect("Tauri config JSON");
        if let Some(resources) = value["bundle"]["resources"].as_object() {
            for destination in resources.values() {
                let destination = destination.as_str().expect("resource destination");
                installed.insert(destination.trim_end_matches('/').to_owned());
            }
        }
        if let Some(binaries) = value["bundle"]["externalBin"].as_array() {
            for binary in binaries {
                let name = binary
                    .as_str()
                    .and_then(|path| path.rsplit('/').next())
                    .expect("sidecar name");
                installed.insert(format!("{name}.exe"));
            }
        }
    }
    let unit_only = ["LICENSE", "NOTICE"];
    let layout: BTreeSet<String> = LAYOUT
        .iter()
        .map(|placement| placement.path.to_owned())
        .filter(|path| !unit_only.contains(&path.as_str()))
        .collect();
    assert_eq!(layout, installed);
}

#[cfg(windows)]
mod round_trip {
    use hypercolor_platform_fs::{WindowsDirectory, WindowsDirectoryAccess, WindowsSid};

    use super::{Fixture, fs};
    use crate::windows::WindowsOwnershipPolicy;
    use crate::windows::payload::{
        retain_installed_release_unit, stage_release_payload, validate_release_payload,
    };
    use crate::windows::store::InstallStore;

    #[test]
    fn a_produced_unit_is_staged_and_retained_by_the_store() {
        let fixture = Fixture::new();
        let output = fixture.output("unit");
        let unit = super::produce_windows_unit(&fixture.sources, &output).expect("produce");
        let source =
            WindowsDirectory::open(&output, WindowsDirectoryAccess::Read).expect("open unit");
        validate_release_payload(&source, unit.unit_id()).expect("the payload validator accepts");

        let release = fixture.base.join("Program Files").join("Hypercolor");
        let state = fixture
            .base
            .join("ProgramData")
            .join("Hypercolor")
            .join("update");
        fs::create_dir_all(&release).expect("release root");
        fs::create_dir_all(&state).expect("state root");
        let store = InstallStore::with_roots(&release, &state, 64 * 1024)
            .expect("store")
            .with_ownership_policy(WindowsOwnershipPolicy::trusting([
                WindowsSid::current_user().expect("current user"),
            ]));
        let lock = store.acquire_lock().expect("lock");
        let staged = stage_release_payload(&store, &lock, &source, unit.unit_id()).expect("stage");
        assert_eq!(staged.id(), unit.unit_id());
        let retained =
            retain_installed_release_unit(&store, &lock, unit.unit_id()).expect("retain");
        assert_eq!(retained, staged);
        assert_eq!(
            fs::read(
                store
                    .unit_path(unit.unit_id())
                    .join("hypercolor-daemon.exe")
            )
            .expect("staged daemon"),
            fs::read(output.join("hypercolor-daemon.exe")).expect("produced daemon")
        );
    }
}
