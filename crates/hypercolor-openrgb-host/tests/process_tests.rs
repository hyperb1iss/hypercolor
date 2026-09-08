use std::path::PathBuf;

use hypercolor_openrgb_host::{
    BinaryKind, HostError, LOOPBACK_HOST, ManagedConfigDir, OpenRgbBinary, ProcessSpec,
    SERVER_LOG_LEVEL, server_args, server_command,
};

fn config_dir() -> ManagedConfigDir {
    ManagedConfigDir {
        root: PathBuf::from("/home/bliss/.local/share/hypercolor/openrgb"),
    }
}

#[test]
fn native_server_command_has_exact_argument_order_and_loopback_host() {
    let binary = OpenRgbBinary {
        path: PathBuf::from("/usr/bin/openrgb"),
        kind: BinaryKind::Native,
        version: Some("1.0rc3".to_owned()),
    };
    let spec = server_command(&binary, &config_dir(), 6742).expect("utf-8 path");
    assert_eq!(spec.program, PathBuf::from("/usr/bin/openrgb"));
    assert_eq!(
        spec.args,
        vec![
            "--server",
            "--server-host",
            "127.0.0.1",
            "--server-port",
            "6742",
            "--noautoconnect",
            "--config",
            "/home/bliss/.local/share/hypercolor/openrgb",
            "--loglevel",
            "4",
        ]
    );
    assert_eq!(LOOPBACK_HOST, "127.0.0.1");
    assert_eq!(SERVER_LOG_LEVEL, 4);
    assert!(spec.env.is_empty());
    assert!(spec.cwd.is_none());
    assert!(
        !spec.args.iter().any(|arg| arg.contains("0.0.0.0")),
        "the managed server must never bind all interfaces"
    );
}

#[test]
fn appimage_uses_the_same_arguments_as_native() {
    let binary = OpenRgbBinary {
        path: PathBuf::from("/home/bliss/Applications/OpenRGB_1.0rc3_x86_64.AppImage"),
        kind: BinaryKind::AppImage,
        version: None,
    };
    let spec = server_command(&binary, &config_dir(), 6800).expect("utf-8 path");
    assert_eq!(spec.program, binary.path);
    assert_eq!(
        spec.args,
        server_args("/home/bliss/.local/share/hypercolor/openrgb", 6800)
    );
    assert_eq!(spec.args[4], "6800");
}

#[test]
fn flatpak_runs_through_flatpak_with_filesystem_grant() {
    let binary = OpenRgbBinary {
        path: PathBuf::from("/usr/bin/flatpak"),
        kind: BinaryKind::Flatpak,
        version: Some("1.0rc3".to_owned()),
    };
    let spec = server_command(&binary, &config_dir(), 6742).expect("utf-8 path");
    assert_eq!(spec.program, PathBuf::from("/usr/bin/flatpak"));
    assert_eq!(spec.args[0], "run");
    assert_eq!(
        spec.args[1],
        "--filesystem=/home/bliss/.local/share/hypercolor/openrgb"
    );
    assert_eq!(spec.args[2], "org.openrgb.OpenRGB");
    assert_eq!(
        &spec.args[3..],
        server_args("/home/bliss/.local/share/hypercolor/openrgb", 6742).as_slice()
    );
}

#[test]
fn process_spec_round_trips_through_serde() {
    let binary = OpenRgbBinary {
        path: PathBuf::from("/usr/bin/openrgb"),
        kind: BinaryKind::Native,
        version: None,
    };
    let spec = server_command(&binary, &config_dir(), 6742).expect("utf-8 path");
    let json = serde_json::to_string(&spec).expect("serialize");
    let back: ProcessSpec = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, spec);

    let minimal: ProcessSpec =
        serde_json::from_str(r#"{"program":"openrgb"}"#).expect("defaults fill in");
    assert!(minimal.args.is_empty());
    assert!(minimal.env.is_empty());
    assert!(minimal.cwd.is_none());
}

#[test]
fn flatpak_rejects_config_paths_with_colons() {
    let binary = OpenRgbBinary {
        path: PathBuf::from("/usr/bin/flatpak"),
        kind: BinaryKind::Flatpak,
        version: None,
    };
    let dir = ManagedConfigDir {
        root: PathBuf::from("/home/bliss/odd:dir/openrgb"),
    };
    let error = server_command(&binary, &dir, 6742).expect_err("colon must be rejected");
    assert!(matches!(
        error,
        HostError::UnsupportedConfigPath { reason, .. } if reason.contains(':')
    ));

    let native = OpenRgbBinary {
        path: PathBuf::from("/usr/bin/openrgb"),
        kind: BinaryKind::Native,
        version: None,
    };
    assert!(
        server_command(&native, &dir, 6742).is_ok(),
        "native launches pass the path through untouched"
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_config_paths_are_rejected() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let binary = OpenRgbBinary {
        path: PathBuf::from("/usr/bin/openrgb"),
        kind: BinaryKind::Native,
        version: None,
    };
    let dir = ManagedConfigDir {
        root: PathBuf::from(OsStr::from_bytes(b"/home/bliss/\xff\xfe/openrgb")),
    };
    let error = server_command(&binary, &dir, 6742).expect_err("non-UTF-8 must be rejected");
    assert!(matches!(
        error,
        HostError::UnsupportedConfigPath { reason, .. } if reason.contains("UTF-8")
    ));
}
