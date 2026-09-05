use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use hypercolor_platform_fs::{DirectoryAuthority, ExclusiveDirectory};
use sha2::{Digest as _, Sha256};

use super::legacy::{
    LegacyBudget, LegacyFile, LegacyLimits, collect_public_legacy_inventory,
    collect_public_legacy_inventory_with, legacy_identity_digest, prepare_legacy_files,
};
use crate::install::linux::legacy_validation::{
    populate_legacy_stage, validate_legacy_snapshot_binding, validate_legacy_unit,
    validate_legacy_unit_with_budget,
};
use crate::install::{
    InstallStore, LINUX_LAYOUT_ITEMS, LinuxExactEntry, LinuxLayoutItem, LinuxLegacySnapshot,
    LinuxPublicTree, UnitId,
};

#[test]
fn existing_legacy_snapshot_requires_the_complete_exact_tree() {
    let (temp, unit, files) = fixture();
    validate_legacy_unit(&unit, &files).expect("complete snapshot");

    fs::remove_file(temp.path().join("legacy/bin/hypercolor-daemon")).expect("remove member");
    assert!(validate_legacy_unit(&unit, &files).is_err());

    let (stale_temp, unit, files) = fixture();
    fs::write(
        stale_temp.path().join("legacy/bin/hypercolor-daemon"),
        b"stale",
    )
    .expect("stale member");
    assert!(validate_legacy_unit(&unit, &files).is_err());

    let (mode_temp, unit, files) = fixture();
    fs::set_permissions(
        mode_temp.path().join("legacy/bin/hypercolor-daemon"),
        fs::Permissions::from_mode(0o644),
    )
    .expect("wrong mode");
    assert!(validate_legacy_unit(&unit, &files).is_err());

    let (extra_temp, unit, files) = fixture();
    fs::write(extra_temp.path().join("legacy/extra"), b"extra").expect("extra member");
    assert!(validate_legacy_unit(&unit, &files).is_err());
}

#[test]
fn public_inventory_captures_complete_historical_owned_leaves_only() {
    let (temp, tree) = public_tree(&[
        (".local/bin/hyper", b"old-cli"),
        (".local/bin/hypercolor-tray", b"old-tray"),
        (".local/bin/unrelated", b"bin-sentinel"),
        (".local/share/bash-completion/completions/hyper", b"bash"),
        (
            ".local/share/bash-completion/completions/unrelated",
            b"bash-sentinel",
        ),
        (".local/share/zsh/site-functions/_hyper", b"zsh"),
        (
            ".local/share/zsh/site-functions/_unrelated",
            b"zsh-sentinel",
        ),
        (".local/share/hypercolor/ui/index.html", b"index"),
        (".local/share/hypercolor/ui/assets/app.js", b"script"),
        (
            ".local/share/icons/hicolor/scalable/apps/hypercolor-symbolic.svg",
            b"icon",
        ),
        (
            ".local/share/icons/hicolor/48x48/apps/hypercolor.png",
            b"icon-48",
        ),
        (
            ".local/share/icons/hicolor/128x128/apps/hypercolor.png",
            b"icon-128",
        ),
        (
            ".local/share/icons/hicolor/256x256/apps/hypercolor.png",
            b"icon-256",
        ),
        (
            ".local/share/icons/hicolor/scalable/apps/unrelated.svg",
            b"other",
        ),
        (".config/fish/completions/hypercolor.fish", b"fish"),
        (".config/fish/completions/hyper.fish", b"old-fish"),
        (".config/fish/completions/unrelated.fish", b"fish-sentinel"),
    ]);
    let inventory = collect_public_legacy_inventory(&tree).expect("legacy inventory");
    assert_eq!(
        inventory
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        [
            "bin/hyper",
            "bin/hypercolor-tray",
            "home/.config/fish/completions/hyper.fish",
            "home/.config/fish/completions/hypercolor.fish",
            "share/bash-completion/completions/hyper",
            "share/hypercolor/ui/assets/app.js",
            "share/hypercolor/ui/index.html",
            "share/icons/hicolor/scalable/apps/hypercolor-symbolic.svg",
            "share/zsh/site-functions/_hyper",
        ]
    );
    assert!(temp.path().join("home").is_dir());
}

#[test]
fn fixed_icon_links_belong_only_to_layout_inspection_not_legacy_extras() {
    let (temp, tree) = public_tree(&[
        (
            ".local/share/icons/hicolor/48x48/apps/hypercolor.png",
            b"48",
        ),
        (
            ".local/share/icons/hicolor/128x128/apps/hypercolor.png",
            b"128",
        ),
        (
            ".local/share/icons/hicolor/256x256/apps/hypercolor.png",
            b"256",
        ),
    ]);
    let home = temp.path().join("home");
    for size in [48, 128, 256] {
        let relative = format!("share/icons/hicolor/{size}x{size}/apps/hypercolor.png");
        let public = home.join(".local").join(&relative);
        fs::remove_file(&public).unwrap();
        std::os::unix::fs::symlink(
            home.join(".local/lib/hypercolor/active").join(relative),
            public,
        )
        .unwrap();
    }
    assert!(
        collect_public_legacy_inventory(&tree)
            .expect("fixed links are inspected by layout owner")
            .is_empty()
    );
    let extra = home.join(".local/share/icons/hicolor/48x48/apps/hypercolor-extra.png");
    std::os::unix::fs::symlink("/foreign/extra.png", extra).unwrap();
    assert!(
        collect_public_legacy_inventory(&tree).is_err(),
        "historical extra owned links remain unsupported"
    );
}

#[test]
fn fixed_icons_are_snapshotted_once_alongside_historical_extras() {
    let (temp, tree) = public_tree(&[
        (".local/bin/hypercolor-daemon", b"daemon"),
        (
            ".local/share/icons/hicolor/48x48/apps/hypercolor.png",
            b"icon-48",
        ),
        (
            ".local/share/icons/hicolor/128x128/apps/hypercolor.png",
            b"icon-128",
        ),
        (
            ".local/share/icons/hicolor/256x256/apps/hypercolor.png",
            b"icon-256",
        ),
        (
            ".local/share/icons/hicolor/scalable/apps/hypercolor-symbolic.svg",
            b"extra",
        ),
    ]);
    fs::set_permissions(
        temp.path().join("home/.local/bin/hypercolor-daemon"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("daemon mode");
    let inventory = collect_public_legacy_inventory(&tree).expect("extras inventory");
    assert_eq!(inventory.len(), 1);
    let mut layout = LINUX_LAYOUT_ITEMS
        .into_iter()
        .map(|item| (item, LinuxExactEntry::Absent))
        .collect::<std::collections::BTreeMap<_, _>>();
    for (item, contents, mode) in [
        (
            LinuxLayoutItem::HypercolorDaemon,
            b"daemon".as_slice(),
            0o755,
        ),
        (LinuxLayoutItem::Icon48, b"icon-48".as_slice(), 0o644),
        (LinuxLayoutItem::Icon128, b"icon-128".as_slice(), 0o644),
        (LinuxLayoutItem::Icon256, b"icon-256".as_slice(), 0o644),
    ] {
        layout.insert(
            item,
            LinuxExactEntry::RegularFile {
                mode,
                sha256: format!("{:x}", Sha256::digest(contents)),
                snapshot_unit: None,
                snapshot_path: None,
            },
        );
    }
    let identity = legacy_identity_digest(&LinuxExactEntry::Absent, &[], layout.iter(), &inventory)
        .expect("unique legacy identity");
    let snapshot = LinuxLegacySnapshot {
        unit: UnitId::new(format!("legacy-{identity}")).expect("legacy unit"),
        version: "legacy".to_owned(),
        launcher: None,
        layout: layout.into_iter().collect(),
        inventory,
    };
    let files = prepare_legacy_files(&snapshot, &tree).expect("complete legacy snapshot");
    let paths = files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(paths.len(), files.len());
    assert!(paths.contains("share/icons/hicolor/48x48/apps/hypercolor.png"));
    assert!(paths.contains("share/icons/hicolor/128x128/apps/hypercolor.png"));
    assert!(paths.contains("share/icons/hicolor/256x256/apps/hypercolor.png"));
    assert!(paths.contains("share/icons/hicolor/scalable/apps/hypercolor-symbolic.svg"));

    let synthetic = tempfile::tempdir().expect("synthetic fixture");
    let exclusive =
        ExclusiveDirectory::try_acquire(synthetic.path(), Path::new(".synthetic-install.lock"))
            .expect("synthetic lock")
            .expect("exclusive synthetic lock");
    let root = exclusive
        .root_directory()
        .expect("synthetic root")
        .create_child_directory(Path::new("legacy"))
        .expect("synthetic unit");
    populate_legacy_stage(&root, &files).expect("populate complete synthetic unit");
    validate_legacy_snapshot_binding(&root, &snapshot.unit).expect("self-bound synthetic unit");
    let foreign = UnitId::new(format!("legacy-{}", "0".repeat(64))).expect("foreign unit");
    assert!(validate_legacy_snapshot_binding(&root, &foreign).is_err());
}

#[test]
fn public_inventory_enforces_each_global_bound_before_copy() {
    let (_temp, tree) = public_tree(&[(".local/share/hypercolor/ui/deep/child/file", b"x")]);
    assert!(
        collect_public_legacy_inventory_with(
            &tree,
            &mut budget(LegacyLimits {
                depth: 1,
                members: 100,
                file_bytes: 100,
                total_bytes: 100,
            }),
        )
        .is_err()
    );

    let (_temp, tree) = public_tree(&[
        (".local/share/hypercolor/ui/a", b"x"),
        (".local/share/hypercolor/ui/b", b"x"),
        (".local/share/hypercolor/ui/c", b"x"),
    ]);
    assert!(
        collect_public_legacy_inventory_with(
            &tree,
            &mut budget(LegacyLimits {
                depth: 16,
                members: 2,
                file_bytes: 100,
                total_bytes: 100,
            }),
        )
        .is_err()
    );

    let (_temp, tree) = public_tree(&[(".local/share/hypercolor/ui/large", b"large")]);
    assert!(
        collect_public_legacy_inventory_with(
            &tree,
            &mut budget(LegacyLimits {
                depth: 16,
                members: 100,
                file_bytes: 4,
                total_bytes: 100,
            }),
        )
        .is_err()
    );

    let (_temp, tree) = public_tree(&[
        (".local/share/hypercolor/ui/a", b"abc"),
        (".local/share/hypercolor/ui/b", b"def"),
    ]);
    assert!(
        collect_public_legacy_inventory_with(
            &tree,
            &mut budget(LegacyLimits {
                depth: 16,
                members: 100,
                file_bytes: 3,
                total_bytes: 5,
            }),
        )
        .is_err()
    );
}

#[test]
fn existing_synthetic_validation_enforces_each_global_bound() {
    let limits = [
        LegacyLimits {
            depth: 1,
            members: 100,
            file_bytes: 100,
            total_bytes: 100,
        },
        LegacyLimits {
            depth: 16,
            members: 1,
            file_bytes: 100,
            total_bytes: 100,
        },
        LegacyLimits {
            depth: 16,
            members: 100,
            file_bytes: 5,
            total_bytes: 100,
        },
        LegacyLimits {
            depth: 16,
            members: 100,
            file_bytes: 100,
            total_bytes: 10,
        },
    ];
    for limit in limits {
        let (_temp, unit, files) = fixture();
        assert!(
            validate_legacy_unit_with_budget(&unit, &files, budget(limit)).is_err(),
            "synthetic bound unexpectedly accepted"
        );
    }
}

fn budget(limits: LegacyLimits) -> LegacyBudget {
    LegacyBudget::with_limits(limits)
}

fn public_tree(files: &[(&str, &[u8])]) -> (tempfile::TempDir, LinuxPublicTree) {
    let temp = tempfile::Builder::new()
        .prefix("linux-public-inventory-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("public fixture");
    let home = temp.path().join("home");
    fs::create_dir(&home).expect("home");
    for (path, contents) in files {
        let destination = home.join(path);
        fs::create_dir_all(destination.parent().expect("file parent")).expect("parents");
        fs::write(destination, contents).expect("public file");
    }
    let store = InstallStore::new(temp.path().join("store"), 64 * 1024);
    let lock = store.acquire_lock().expect("install lock");
    let tree = LinuxPublicTree::new(&lock, &home).expect("public tree");
    (temp, tree)
}

fn fixture() -> (tempfile::TempDir, DirectoryAuthority, Vec<LegacyFile>) {
    let temp = tempfile::tempdir().expect("fixture");
    let exclusive = ExclusiveDirectory::try_acquire(temp.path(), Path::new(".install.lock"))
        .expect("lock")
        .expect("exclusive lock");
    let units = exclusive.root_directory().expect("root authority");
    let unit = units
        .create_child_directory(Path::new("legacy"))
        .expect("legacy unit");
    let files = vec![
        LegacyFile {
            path: "bin/hypercolor-daemon".to_owned(),
            mode: 0o755,
            contents: b"daemon".to_vec(),
        },
        LegacyFile {
            path: "home/.config/fish/completions/hypercolor.fish".to_owned(),
            mode: 0o600,
            contents: b"fish".to_vec(),
        },
        LegacyFile {
            path: "manifest.json".to_owned(),
            mode: 0o644,
            contents: b"manifest".to_vec(),
        },
        LegacyFile {
            path: "share/hypercolor/ui/assets/app.js".to_owned(),
            mode: 0o644,
            contents: b"script".to_vec(),
        },
        LegacyFile {
            path: "share/icons/hicolor/scalable/apps/hypercolor-symbolic.svg".to_owned(),
            mode: 0o644,
            contents: b"icon".to_vec(),
        },
    ];
    populate_legacy_stage(&unit, &files).expect("populate snapshot");
    (temp, unit, files)
}
