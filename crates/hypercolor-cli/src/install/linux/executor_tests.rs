use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::net::UnixListener;
use std::path::Path;

use hypercolor_platform_fs::ExclusiveDirectory;

use super::LinuxInstallPlatform;
use super::directory::read_opened_public_bytes;
use super::effects::autostart_operation;
use super::executor::{LinuxNativeExecutor, SERVICE, systemctl_command};
use super::model::{LinuxInstallConfig, parse_systemd_show};
use super::runtime::LinuxSystemdConnection;
use crate::install::InstallStore;

fn with_native_public_tree(
    prepare: impl FnOnce(&Path),
    check: impl FnOnce(&Path, &mut LinuxNativeExecutor),
) {
    let fixture = tempfile::Builder::new()
        .prefix("linux-native-public-tree-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("fixture");
    let home = fixture.path().join("home");
    fs::create_dir(&home).expect("home");
    prepare(&home);
    let runtime = tempfile::tempdir().expect("runtime");
    fs::set_permissions(runtime.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let _bus = UnixListener::bind(runtime.path().join("bus")).unwrap();
    let uid = fs::metadata(runtime.path()).unwrap().uid();
    let connection = LinuxSystemdConnection::from_runtime_directory(runtime.path(), uid).unwrap();
    let store = InstallStore::new(fixture.path().join("store"), 64 * 1024);
    let lock = store.acquire_lock().unwrap();
    let tree = super::directory::LinuxPublicTree::new(&lock, &home).unwrap();
    let mut executor = LinuxNativeExecutor::new_with_connection(
        &store,
        &lock,
        tree,
        "127.0.0.1:9420".parse().unwrap(),
        connection,
    )
    .unwrap();
    check(&home, &mut executor);
}

#[test]
fn native_public_reads_accept_virgin_home_then_ordered_scaffold_creation() {
    use super::executor::LinuxInstallExecutor as _;
    use super::model::{
        LINUX_DIRECTORY_ITEMS, LINUX_LAYOUT_ITEMS, LinuxDirectoryState, LinuxExactEntry,
    };
    with_native_public_tree(
        |_| {},
        |home, executor| {
            assert_eq!(
                executor.launcher_entry(4096).unwrap(),
                (LinuxExactEntry::Absent, vec![])
            );
            for item in LINUX_LAYOUT_ITEMS {
                assert_eq!(
                    executor.layout_entry(item).unwrap(),
                    LinuxExactEntry::Absent
                );
            }
            assert_eq!(
                fs::read_dir(home).unwrap().count(),
                0,
                "inspection must not create scaffolding"
            );
            assert!(
                executor
                    .replace_launcher(&LinuxExactEntry::Absent, None)
                    .is_err()
            );
            for item in LINUX_DIRECTORY_ITEMS {
                executor
                    .replace_directory(item, LinuxDirectoryState::Absent, true)
                    .unwrap();
            }
            assert_eq!(
                executor.launcher_entry(4096).unwrap(),
                (LinuxExactEntry::Absent, vec![])
            );
            for item in LINUX_LAYOUT_ITEMS {
                assert_eq!(
                    executor.layout_entry(item).unwrap(),
                    LinuxExactEntry::Absent
                );
            }
            fs::write(
                home.join(".config/systemd/user/hypercolor.service"),
                b"foreign launcher",
            )
            .unwrap();
            let (entry, bytes) = executor.launcher_entry(4096).unwrap();
            assert!(matches!(entry, LinuxExactEntry::RegularFile { .. }));
            assert_eq!(bytes, b"foreign launcher");
        },
    );
}

#[test]
fn native_public_absence_rejects_appeared_directory_or_symlink_ancestors() {
    use super::executor::LinuxInstallExecutor as _;
    use super::model::LINUX_LAYOUT_ITEMS;
    for symlink in [false, true] {
        with_native_public_tree(
            |_| {},
            |home, executor| {
                for name in [".config", ".local"] {
                    if symlink {
                        std::os::unix::fs::symlink(home, home.join(name)).unwrap();
                    } else {
                        fs::create_dir(home.join(name)).unwrap();
                    }
                }
                assert!(executor.launcher_entry(4096).is_err());
                for item in LINUX_LAYOUT_ITEMS {
                    assert!(executor.layout_entry(item).is_err(), "{item:?}");
                }
            },
        );
    }
}

#[test]
fn native_public_reads_reject_replaced_present_parent_and_malformed_launcher() {
    use super::executor::LinuxInstallExecutor as _;
    for symlink in [false, true] {
        with_native_public_tree(
            |home| fs::create_dir_all(home.join(".config/systemd/user")).unwrap(),
            |home, executor| {
                let launcher = home.join(".config/systemd/user/hypercolor.service");
                fs::create_dir(&launcher).unwrap();
                assert!(executor.launcher_entry(4096).is_err());
                fs::rename(home.join(".config"), home.join("old-config")).unwrap();
                if symlink {
                    std::os::unix::fs::symlink(home.join("old-config"), home.join(".config"))
                        .unwrap();
                } else {
                    fs::create_dir_all(home.join(".config/systemd/user")).unwrap();
                }
                assert!(executor.launcher_entry(4096).is_err());
            },
        );
    }
}

#[test]
fn native_topology_rejects_a_fragment_outside_the_retained_home() {
    let fixture = tempfile::Builder::new()
        .prefix("linux-native-topology-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("fixture");
    let runtime_fixture = tempfile::tempdir().expect("runtime fixture");
    let home = fixture.path().join("home");
    let runtime = runtime_fixture.path();
    fs::create_dir(&home).expect("home");
    fs::set_permissions(runtime, fs::Permissions::from_mode(0o700)).expect("runtime mode");
    let _bus = UnixListener::bind(runtime.join("bus")).expect("bus socket");
    let uid = fs::metadata(runtime).expect("runtime metadata").uid();
    let connection =
        LinuxSystemdConnection::from_runtime_directory(runtime, uid).expect("connection");
    let store = InstallStore::new(fixture.path().join("store"), 64 * 1024);
    let lock = store.acquire_lock().expect("install lock");
    let tree = super::directory::LinuxPublicTree::new(&lock, &home).expect("public tree");
    let executor = LinuxNativeExecutor::new_with_connection(
        &store,
        &lock,
        tree,
        "127.0.0.1:9420".parse().expect("HTTP address"),
        connection,
    )
    .expect("native executor");
    let config = LinuxInstallConfig {
        direct_fragment_path: fixture
            .path()
            .join("foreign/.config/systemd/user/hypercolor.service")
            .to_str()
            .expect("UTF-8 path")
            .to_owned(),
        immutable_units_root: store.root().join("units"),
        active_root: store.active_path(),
    };

    let error = LinuxInstallPlatform::new(executor, config, [])
        .err()
        .expect("split launcher authority must fail before inspection");

    assert!(error.to_string().contains("native store authority"));
}

#[test]
fn native_constructor_rejects_canonical_store_path_replacement() {
    let fixture = tempfile::Builder::new()
        .prefix("linux-native-store-replacement-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("fixture");
    let runtime_fixture = tempfile::tempdir().expect("runtime fixture");
    let home = fixture.path().join("home");
    let runtime = runtime_fixture.path();
    fs::create_dir(&home).expect("home");
    fs::set_permissions(runtime, fs::Permissions::from_mode(0o700)).expect("runtime mode");
    let _bus = UnixListener::bind(runtime.join("bus")).expect("bus socket");
    let uid = fs::metadata(runtime).expect("runtime metadata").uid();
    let connection =
        LinuxSystemdConnection::from_runtime_directory(runtime, uid).expect("connection");
    let store = InstallStore::new(home.join(".local/lib/hypercolor"), 64 * 1024);
    let lock = store
        .acquire_anchored_lock(&home)
        .expect("anchored install lock");
    let tree = super::directory::LinuxPublicTree::new(&lock, &home).expect("public tree");
    let displaced = home.join(".local/lib/displaced-hypercolor");
    fs::rename(store.root(), &displaced).expect("displace retained store");
    fs::create_dir(store.root()).expect("create replacement store");
    fs::write(store.root().join("sentinel"), b"attacker").expect("replacement sentinel");

    let error = LinuxNativeExecutor::new_with_connection(
        &store,
        &lock,
        tree,
        "127.0.0.1:9420".parse().expect("HTTP address"),
        connection,
    )
    .expect_err("canonical replacement must fail before native inspection");

    assert!(error.to_string().contains("retained store inode"));
    assert_eq!(
        fs::read(store.root().join("sentinel")).expect("read replacement sentinel"),
        b"attacker"
    );
    assert!(!store.root().join("units").exists());
    assert!(!store.root().join("active").exists());
}

#[test]
fn systemctl_command_has_only_fixed_locale_and_bound_connection_environment() {
    let fixture = tempfile::tempdir().expect("fixture");
    fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o700)).expect("runtime mode");
    let _bus = UnixListener::bind(fixture.path().join("bus")).expect("bus socket");
    let uid = fs::metadata(fixture.path()).expect("metadata").uid();
    let connection =
        LinuxSystemdConnection::from_runtime_directory(fixture.path(), uid).expect("connection");
    let command = systemctl_command(&connection, &["show", SERVICE]);
    let environment = command
        .get_envs()
        .map(|(name, value)| {
            (
                name.to_os_string(),
                value.expect("set environment value").to_os_string(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        environment,
        BTreeMap::from([
            (OsString::from("LANG"), OsString::from("C")),
            (OsString::from("LC_ALL"), OsString::from("C")),
            (
                OsString::from("XDG_RUNTIME_DIR"),
                fixture.path().as_os_str().to_os_string(),
            ),
        ])
    );
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        args,
        [
            "--signal=KILL",
            "10s",
            "/usr/bin/systemctl",
            "--user",
            "--no-ask-password",
            "show",
            SERVICE,
        ]
    );
}

#[test]
fn systemd_absent_unit_accepts_missing_service_property_only() {
    let captured = "MainPID=0\nLoadState=not-found\nActiveState=inactive\nSubState=dead\nFragmentPath=\nUnitFileState=\nInvocationID=\n";
    let observation = parse_systemd_show(captured.as_bytes()).expect("absent systemd unit");
    assert_eq!(observation.load_state, "not-found");
    assert!(observation.exec_start.is_empty());

    for missing in [
        "MainPID=0\n",
        "LoadState=not-found\n",
        "ActiveState=inactive\n",
        "SubState=dead\n",
        "FragmentPath=\n",
        "UnitFileState=\n",
        "InvocationID=\n",
    ] {
        assert!(parse_systemd_show(captured.replace(missing, "").as_bytes()).is_err());
    }
    for (field, contradictory) in [
        ("LoadState=not-found", "LoadState=loaded"),
        ("ActiveState=inactive", "ActiveState=active"),
        ("FragmentPath=", "FragmentPath=/foreign.service"),
        ("UnitFileState=", "UnitFileState=enabled"),
    ] {
        assert!(parse_systemd_show(captured.replace(field, contradictory).as_bytes()).is_err());
    }
    let loaded = "MainPID=0\nLoadState=loaded\nActiveState=inactive\nSubState=dead\nFragmentPath=/home/test/.config/systemd/user/hypercolor.service\nUnitFileState=enabled\nInvocationID=\n";
    let error = parse_systemd_show(loaded.as_bytes()).expect_err("loaded unit needs ExecStart");
    assert!(error.to_string().contains("missing required fields"));
}

#[test]
fn absent_disable_emits_no_command_but_loaded_enablement_does() {
    let absent = parse_systemd_show(
        b"LoadState=not-found\nActiveState=inactive\nSubState=dead\nUnitFileState=\nFragmentPath=\nExecStart=\nMainPID=0\nInvocationID=\n",
    )
    .expect("native absent observation");
    assert_eq!(autostart_operation(false, &absent), None);

    let loaded = parse_systemd_show(
        b"LoadState=loaded\nActiveState=inactive\nSubState=dead\nUnitFileState=enabled\nFragmentPath=/home/test/.config/systemd/user/hypercolor.service\nExecStart={ path=/daemon ; argv[]=/daemon ; ignore_errors=no ; start_time=[n/a] ; stop_time=[n/a] ; pid=0 ; code=(null) ; status=0/0 }\nMainPID=0\nInvocationID=\n",
    )
    .expect("native loaded observation");
    assert_eq!(autostart_operation(false, &loaded), Some("disable"));
}

#[test]
fn public_exact_read_rejects_growth_after_the_metadata_bound() {
    let fixture = tempfile::Builder::new()
        .prefix("linux-public-read-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("fixture");
    let public = fixture.path().join("public");
    let lock_root = fixture.path().join("lock");
    fs::create_dir(&public).expect("public directory");
    fs::create_dir(&lock_root).expect("lock directory");
    let entry = public.join("entry");
    fs::write(&entry, b"fixed").expect("public entry");
    let exclusive = ExclusiveDirectory::try_acquire(&lock_root, Path::new("install.lock"))
        .expect("lock")
        .expect("exclusive lock");
    let authority = exclusive
        .open_public_directory(&public)
        .expect("public authority");
    let mut opened = authority
        .open_regular_file(Path::new("entry"))
        .expect("opened entry");
    let initial_size = opened.metadata().size();
    OpenOptions::new()
        .append(true)
        .open(&entry)
        .expect("append handle")
        .write_all(b"-growth")
        .expect("grow public entry");

    let error = read_opened_public_bytes(&mut opened, initial_size, 1024)
        .expect_err("growth beyond the initial bound must fail");

    assert!(error.to_string().contains("changed size"));
}
