use std::ffi::OsString;
use std::path::Path;

use hypercolor_openrgb_host::{
    BinaryKind, classify_binary, executable_names, find_appimage_in, find_in_path,
    is_executable_file, parse_flatpak_info_version, parse_version_output, read_version,
};

fn write_executable(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write executable");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod +x");
    }
}

fn path_value(dirs: &[&Path]) -> OsString {
    std::env::join_paths(dirs).expect("join PATH entries")
}

#[test]
fn find_in_path_walks_entries_in_order_and_skips_non_executables() {
    let first = tempfile::tempdir().expect("tempdir");
    let second = tempfile::tempdir().expect("tempdir");
    let name = executable_names()[0];
    std::fs::write(first.path().join(name), "not executable").expect("write plain file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            first.path().join(name),
            std::fs::Permissions::from_mode(0o644),
        )
        .expect("chmod -x");
    }
    write_executable(&second.path().join(name), "#!/bin/sh\nexit 0\n");

    let path = path_value(&[first.path(), second.path()]);
    let found = find_in_path(executable_names(), Some(&path)).expect("should find the executable");
    #[cfg(unix)]
    assert_eq!(
        found,
        second.path().join(name),
        "non-executable entry is skipped"
    );
    #[cfg(not(unix))]
    assert_eq!(found, first.path().join(name), "any file counts off unix");
}

#[test]
fn find_in_path_prefers_earlier_entries_and_handles_missing_path() {
    let first = tempfile::tempdir().expect("tempdir");
    let second = tempfile::tempdir().expect("tempdir");
    let name = executable_names()[0];
    write_executable(&first.path().join(name), "#!/bin/sh\nexit 0\n");
    write_executable(&second.path().join(name), "#!/bin/sh\nexit 0\n");

    let path = path_value(&[first.path(), second.path()]);
    assert_eq!(
        find_in_path(executable_names(), Some(&path)),
        Some(first.path().join(name))
    );
    assert_eq!(find_in_path(executable_names(), None), None);
    let empty = tempfile::tempdir().expect("tempdir");
    let path = path_value(&[empty.path()]);
    assert_eq!(find_in_path(executable_names(), Some(&path)), None);
    assert!(
        !is_executable_file(empty.path()),
        "directories are not executables"
    );
}

#[test]
fn classify_recognises_appimages_case_insensitively() {
    assert_eq!(
        classify_binary(Path::new(
            "/home/x/Applications/OpenRGB_1.0rc3_x86_64.AppImage"
        )),
        BinaryKind::AppImage
    );
    assert_eq!(
        classify_binary(Path::new("/home/x/openrgb.appimage")),
        BinaryKind::AppImage
    );
    assert_eq!(
        classify_binary(Path::new("/usr/bin/openrgb")),
        BinaryKind::Native
    );
    assert_eq!(
        classify_binary(Path::new(r"C:\Program Files\OpenRGB\OpenRGB.exe")),
        BinaryKind::Native
    );
}

#[test]
fn find_appimage_in_picks_the_newest_named_openrgb_appimage() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_executable(&dir.path().join("OpenRGB_0.9_x86_64.AppImage"), "");
    write_executable(&dir.path().join("OpenRGB_1.0rc3_x86_64.AppImage"), "");
    write_executable(&dir.path().join("Other_2.0.AppImage"), "");
    std::fs::write(dir.path().join("OpenRGB_notes.txt"), "").expect("write");
    let found = find_appimage_in(dir.path()).expect("appimage found");
    assert_eq!(found, dir.path().join("OpenRGB_1.0rc3_x86_64.AppImage"));
    assert_eq!(find_appimage_in(&dir.path().join("missing")), None);
}

#[test]
fn version_parsing_prefers_the_version_line() {
    let output = "OpenRGB 1.0rc3, for controlling RGB lighting.\n  Version: 1.0rc3 (git 7a1b2c3d)\n  Build Date: 2026-08-01\n";
    assert_eq!(parse_version_output(output).as_deref(), Some("1.0rc3"));
    assert_eq!(
        parse_version_output("OpenRGB 0.9, for controlling RGB lighting.").as_deref(),
        Some("0.9")
    );
    assert_eq!(parse_version_output("no numbers here"), None);
    assert_eq!(parse_version_output(""), None);
}

#[test]
fn flatpak_info_version_parsing() {
    let output = "\nOpenRGB - Open source RGB lighting control\n\n          ID: org.openrgb.OpenRGB\n         Ref: app/org.openrgb.OpenRGB/x86_64/stable\n        Arch: x86_64\n      Branch: stable\n     Version: 1.0rc3\n     License: GPL-2.0-only\n";
    assert_eq!(
        parse_flatpak_info_version(output).as_deref(),
        Some("1.0rc3")
    );
    assert_eq!(parse_flatpak_info_version("ID: org.openrgb.OpenRGB"), None);
}

#[cfg(unix)]
#[tokio::test]
async fn read_version_runs_the_binary_and_tolerates_failures() {
    let dir = tempfile::tempdir().expect("tempdir");
    let good = dir.path().join("openrgb-good");
    write_executable(
        &good,
        "#!/bin/sh\necho 'OpenRGB 1.0rc3, for controlling RGB lighting.'\necho '  Version: 1.0rc3 (git abc)'\n",
    );
    assert_eq!(read_version(&good).await.as_deref(), Some("1.0rc3"));

    let failing = dir.path().join("openrgb-fail");
    write_executable(&failing, "#!/bin/sh\necho 'Version: 9.9' >&2\nexit 3\n");
    assert_eq!(
        read_version(&failing).await,
        None,
        "non-zero exit yields no version"
    );

    assert_eq!(read_version(&dir.path().join("missing")).await, None);
}
