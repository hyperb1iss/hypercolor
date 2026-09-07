//! Per-platform OpenRGB install guidance.
//!
//! [`install_hints`] inspects the host once (which package managers sit on
//! `PATH`) and hands the result to the pure [`install_hints_for`], which is
//! what tests exercise.

use crate::detect::{FLATPAK_APP_ID, find_in_path};
use crate::types::{InstallHint, InstallMethod, Platform};

/// OpenRGB's release download page.
pub const RELEASES_URL: &str = "https://openrgb.org/releases.html";

/// Package managers this crate looks for on Linux `PATH`, in priority order.
const LINUX_PACKAGE_MANAGERS: [(&str, InstallMethod); 5] = [
    ("pacman", InstallMethod::Pacman),
    ("apt", InstallMethod::Apt),
    ("dnf", InstallMethod::Dnf),
    ("zypper", InstallMethod::Zypper),
    ("flatpak", InstallMethod::Flatpak),
];

const LINUX_UDEV_NOTE: &str = "Distro packages install /usr/lib/udev/rules.d/60-openrgb.rules. \
For SMBus devices (motherboard headers, DRAM) also run `sudo modprobe i2c-dev` plus \
`i2c-i801` (Intel) or `i2c-piix4` (AMD), and persist them in /etc/modules-load.d/i2c.conf. \
Some Gigabyte boards need the kernel parameter acpi_enforce_resources=lax.";

const LINUX_SELF_INSTALLED_UDEV_NOTE: &str = "Generate the udev rules yourself: \
`sudo <openrgb> --generate-udev-rules /etc/udev/rules.d/60-openrgb.rules`, then \
`sudo udevadm control --reload-rules && sudo udevadm trigger`. For SMBus devices also load \
i2c-dev plus i2c-i801 (Intel) or i2c-piix4 (AMD).";

const FLATPAK_NOTE: &str = "Runs as `flatpak run org.openrgb.OpenRGB`. Install the udev rules \
with `sudo sh -c 'flatpak run org.openrgb.OpenRGB --print-udev-rules > \
/etc/udev/rules.d/60-openrgb.rules'`, then `sudo udevadm control --reload-rules && sudo \
udevadm trigger`. SMBus devices also need i2c-dev plus i2c-i801 (Intel) or i2c-piix4 (AMD).";

const WINDOWS_NOTE: &str = "HID devices work without elevation. SMBus devices (motherboard \
headers, DRAM) need the PawnIO driver, which OpenRGB 1.0rc2+ uses instead of WinRing0, and \
OpenRGB must run as Administrator or be installed as a service. Hypercolor's native Windows \
SMBus path already uses PawnIO, so a PawnIO install serves both.";

const MACOS_NOTE: &str = "macOS builds are HID only: no SMBus, so motherboard headers and \
DRAM are not reachable. Pick the Intel or Apple Silicon zip to match your Mac.";

/// Detect which Linux package managers are available on `PATH`.
///
/// Returns an empty list on non-Linux hosts.
#[must_use]
pub fn detect_package_managers() -> Vec<InstallMethod> {
    if Platform::current() != Platform::Linux {
        return Vec::new();
    }
    let path = std::env::var_os("PATH");
    LINUX_PACKAGE_MANAGERS
        .iter()
        .filter(|(name, _)| find_in_path(&[name], path.as_deref()).is_some())
        .map(|(_, method)| *method)
        .collect()
}

/// Install hints for the current host, ordered from most to least preferred.
#[must_use]
pub fn install_hints() -> Vec<InstallHint> {
    install_hints_for(Platform::current(), &detect_package_managers())
}

/// Pure hint selection for `platform` given the package managers found.
///
/// Linux honours `available` in [`LINUX_PACKAGE_MANAGERS`] order and always
/// ends with the AppImage download. Windows recommends winget then the MSI.
/// macOS has only the direct download.
#[must_use]
pub fn install_hints_for(platform: Platform, available: &[InstallMethod]) -> Vec<InstallHint> {
    match platform {
        Platform::Linux => {
            let mut hints: Vec<InstallHint> = LINUX_PACKAGE_MANAGERS
                .iter()
                .filter(|(_, method)| available.contains(method))
                .map(|(_, method)| linux_hint(*method))
                .collect();
            hints.push(InstallHint {
                platform,
                method: InstallMethod::DirectDownload,
                command: format!(
                    "{RELEASES_URL} (AppImage: x86_64, arm64, i386, armhf; .deb for Debian \
                     Bookworm/Trixie; Fedora RPM)"
                ),
                note: format!(
                    "`chmod +x OpenRGB*.AppImage` after download. {LINUX_SELF_INSTALLED_UDEV_NOTE}"
                ),
            });
            hints
        }
        Platform::Windows => vec![
            InstallHint {
                platform,
                method: InstallMethod::Winget,
                command: "winget install -e --id OpenRGB.OpenRGB".to_owned(),
                note: WINDOWS_NOTE.to_owned(),
            },
            InstallHint {
                platform,
                method: InstallMethod::DirectDownload,
                command: format!("{RELEASES_URL} (MSI installer or portable zip, x64/x86)"),
                note: WINDOWS_NOTE.to_owned(),
            },
        ],
        Platform::Macos => vec![InstallHint {
            platform,
            method: InstallMethod::DirectDownload,
            command: format!("{RELEASES_URL} (macOS Intel or Apple Silicon zip)"),
            note: MACOS_NOTE.to_owned(),
        }],
    }
}

fn linux_hint(method: InstallMethod) -> InstallHint {
    let (command, note) = match method {
        InstallMethod::Pacman => (
            "sudo pacman -S openrgb".to_owned(),
            format!("Package: extra/openrgb. {LINUX_UDEV_NOTE}"),
        ),
        InstallMethod::Apt => (
            "sudo apt install openrgb".to_owned(),
            format!(
                "Distro repositories may lag; the upstream .deb for Debian Bookworm/Trixie is at \
                 {RELEASES_URL} (`sudo apt install ./openrgb_*.deb`). {LINUX_UDEV_NOTE}"
            ),
        ),
        InstallMethod::Dnf => (
            "sudo dnf install openrgb".to_owned(),
            format!(
                "Distro repositories may lag; the upstream Fedora RPM is at {RELEASES_URL}. \
                 {LINUX_UDEV_NOTE}"
            ),
        ),
        InstallMethod::Zypper => (
            "sudo zypper install openrgb".to_owned(),
            LINUX_UDEV_NOTE.to_owned(),
        ),
        InstallMethod::Flatpak => (
            format!("flatpak install flathub {FLATPAK_APP_ID}"),
            FLATPAK_NOTE.to_owned(),
        ),
        InstallMethod::Winget | InstallMethod::DirectDownload => (
            RELEASES_URL.to_owned(),
            LINUX_SELF_INSTALLED_UDEV_NOTE.to_owned(),
        ),
    };
    InstallHint {
        platform: Platform::Linux,
        method,
        command,
        note,
    }
}
