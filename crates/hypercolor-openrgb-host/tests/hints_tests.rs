use hypercolor_openrgb_host::{InstallMethod, Platform, RELEASES_URL, install_hints_for};

#[test]
fn linux_orders_detected_managers_then_flatpak_then_download() {
    let hints = install_hints_for(
        Platform::Linux,
        &[InstallMethod::Flatpak, InstallMethod::Pacman],
    );
    let methods: Vec<InstallMethod> = hints.iter().map(|hint| hint.method).collect();
    assert_eq!(
        methods,
        vec![
            InstallMethod::Pacman,
            InstallMethod::Flatpak,
            InstallMethod::DirectDownload
        ]
    );
    assert!(hints.iter().all(|hint| hint.platform == Platform::Linux));
    assert_eq!(hints[0].command, "sudo pacman -S openrgb");
    assert!(hints[0].note.contains("extra/openrgb"));
    assert!(hints[0].note.contains("i2c-dev"));
    assert_eq!(
        hints[1].command,
        "flatpak install flathub org.openrgb.OpenRGB"
    );
    assert!(hints[1].note.contains("--print-udev-rules"));
    assert!(hints[2].command.starts_with(RELEASES_URL));
    assert!(hints[2].note.contains("--generate-udev-rules"));
}

#[test]
fn linux_without_package_managers_still_offers_a_download() {
    let hints = install_hints_for(Platform::Linux, &[]);
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].method, InstallMethod::DirectDownload);
    assert!(hints[0].command.contains("AppImage"));
}

#[test]
fn linux_apt_dnf_zypper_commands_are_exact() {
    let hints = install_hints_for(
        Platform::Linux,
        &[
            InstallMethod::Zypper,
            InstallMethod::Dnf,
            InstallMethod::Apt,
        ],
    );
    let commands: Vec<&str> = hints.iter().map(|hint| hint.command.as_str()).collect();
    assert_eq!(commands[0], "sudo apt install openrgb");
    assert_eq!(commands[1], "sudo dnf install openrgb");
    assert_eq!(commands[2], "sudo zypper install openrgb");
    assert!(hints[0].note.contains("Bookworm/Trixie"));
    assert!(hints[1].note.contains("Fedora RPM"));
}

#[test]
fn windows_recommends_winget_then_msi_with_pawnio_note() {
    let hints = install_hints_for(Platform::Windows, &[]);
    assert_eq!(hints.len(), 2);
    assert_eq!(hints[0].method, InstallMethod::Winget);
    assert_eq!(hints[0].command, "winget install -e --id OpenRGB.OpenRGB");
    assert_eq!(hints[1].method, InstallMethod::DirectDownload);
    assert!(hints[1].command.contains("MSI"));
    for hint in &hints {
        assert_eq!(hint.platform, Platform::Windows);
        assert!(hint.note.contains("PawnIO"));
        assert!(hint.note.contains("Administrator"));
        assert!(hint.note.contains("WinRing0"));
    }
}

#[test]
fn windows_ignores_linux_package_managers() {
    let hints = install_hints_for(Platform::Windows, &[InstallMethod::Pacman]);
    assert!(
        hints
            .iter()
            .all(|hint| hint.method != InstallMethod::Pacman)
    );
}

#[test]
fn macos_is_direct_download_and_hid_only() {
    let hints = install_hints_for(Platform::Macos, &[InstallMethod::Flatpak]);
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].method, InstallMethod::DirectDownload);
    assert_eq!(hints[0].platform, Platform::Macos);
    assert!(hints[0].command.contains("Apple Silicon"));
    assert!(hints[0].note.contains("HID only"));
    assert!(hints[0].note.contains("no SMBus"));
}

#[test]
fn hints_serialize_with_snake_case_enums() {
    let hints = install_hints_for(Platform::Windows, &[]);
    let json = serde_json::to_value(&hints[0]).expect("hint should serialize");
    assert_eq!(json["platform"], "windows");
    assert_eq!(json["method"], "winget");
}
