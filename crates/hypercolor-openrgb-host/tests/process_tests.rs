use std::path::PathBuf;

use hypercolor_openrgb_host::{
    BinaryKind, LOOPBACK_HOST, ManagedConfigDir, OpenRgbBinary, ProcessSpec, SERVER_LOG_LEVEL,
    server_args, server_command,
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
    let spec = server_command(&binary, &config_dir(), 6742);
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
    let spec = server_command(&binary, &config_dir(), 6800);
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
    let spec = server_command(&binary, &config_dir(), 6742);
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
    let spec = server_command(&binary, &config_dir(), 6742);
    let json = serde_json::to_string(&spec).expect("serialize");
    let back: ProcessSpec = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, spec);

    let minimal: ProcessSpec =
        serde_json::from_str(r#"{"program":"openrgb"}"#).expect("defaults fill in");
    assert!(minimal.args.is_empty());
    assert!(minimal.env.is_empty());
    assert!(minimal.cwd.is_none());
}
