use std::path::Path;

use super::{command_directory, render, rendered_target, rendered_unit};
use crate::UnitId;

const COMMANDS: &str = "/home/person/.local/bin";

/// The entry `scripts/dist.sh` writes into every release.
const RELEASE_ENTRY: &str = "[Desktop Entry]\n\
Type=Application\n\
Version=1.0\n\
Name=Hypercolor\n\
Comment=Open the Hypercolor control UI\n\
Exec=hypercolor-open\n\
Terminal=false\n\
Categories=Settings;Utility;\n\
Keywords=rgb;lighting;led;\n\
Icon=hypercolor\n\
StartupNotify=true\n";

fn unit(byte: &str) -> UnitId {
    UnitId::new(byte.repeat(32)).expect("unit ID")
}

#[test]
fn a_release_entry_names_the_installations_own_launcher_and_keeps_everything_else() {
    let rendered = String::from_utf8(render(RELEASE_ENTRY.as_bytes(), COMMANDS)).expect("UTF-8");

    assert_eq!(
        rendered,
        RELEASE_ENTRY.replace(
            "Exec=hypercolor-open\n",
            "Exec=/home/person/.local/bin/hypercolor-open\n"
        )
    );
}

#[test]
fn every_published_command_is_rewritten_by_any_path_with_its_arguments() {
    let source = "Exec=/usr/bin/hypercolor-open\n\
TryExec=hypercolor-app\n\
Exec=hypercolor tui --theme dark %U\n\
[Desktop Action daemon]\n\
Exec=/opt/hypercolor/bin/hypercolor-daemon --verbose";

    assert_eq!(
        String::from_utf8(render(source.as_bytes(), COMMANDS)).expect("UTF-8"),
        "Exec=/home/person/.local/bin/hypercolor-open\n\
TryExec=/home/person/.local/bin/hypercolor-app\n\
Exec=/home/person/.local/bin/hypercolor tui --theme dark %U\n\
[Desktop Action daemon]\n\
Exec=/home/person/.local/bin/hypercolor-daemon --verbose"
    );
}

#[test]
fn other_programs_quoted_programs_and_other_keys_stay_exactly_as_shipped() {
    let source = b"Exec=xdg-open http://127.0.0.1:9420/\n\
Exec=\"hypercolor-open\"\n\
Exec = hypercolor-open\n\
Name=hypercolor-open\n\
X-Exec=hypercolor-open\n\
Exec=hypercolor-opener\n\
Exec=hypercolor-open\r\n\
\xff\xfe not text\n";

    assert_eq!(render(source, COMMANDS), source.to_vec());
}

#[test]
fn an_empty_entry_renders_empty() {
    assert!(render(b"", COMMANDS).is_empty());
}

#[test]
fn the_rendered_entry_lives_beside_the_units_under_the_public_entrys_name() {
    let release = Path::new("/home/person/.local/share/hypercolor/releases");
    let id = unit("ab");
    let target = rendered_target(release, &id);

    assert_eq!(
        target,
        format!(
            "/home/person/.local/share/hypercolor/releases/desktop-entries/{}/hypercolor.desktop",
            id.as_str()
        )
    );
    assert_eq!(rendered_unit(release, &target), Some(id));
}

#[test]
fn only_an_exact_rendered_entry_names_a_unit() {
    let release = Path::new("/home/person/.local/share/hypercolor/releases");
    let id = unit("cd");
    let elsewhere = Path::new("/home/person/other/releases");
    for target in [
        format!(
            "{}/active/share/applications/hypercolor.desktop",
            release.display()
        ),
        format!(
            "{}/units/{}/share/applications/hypercolor.desktop",
            release.display(),
            id.as_str()
        ),
        rendered_target(elsewhere, &id),
        format!(
            "{}/desktop-entries/{}/other.desktop",
            release.display(),
            id.as_str()
        ),
        format!("{}/desktop-entries/{}", release.display(), id.as_str()),
        format!(
            "{}/desktop-entries/{}/hypercolor.desktop/x",
            release.display(),
            id.as_str()
        ),
        format!(
            "{}/desktop-entries/not-a-unit/hypercolor.desktop",
            release.display()
        ),
        format!(
            "{}/desktop-entries/{}/../{}/hypercolor.desktop",
            release.display(),
            id.as_str(),
            id.as_str()
        ),
        format!(
            "{}/desktop-entries//{}/hypercolor.desktop",
            release.display(),
            id.as_str()
        ),
        format!(
            "{}/desktop-entries/./{}/hypercolor.desktop",
            release.display(),
            id.as_str()
        ),
    ] {
        assert_eq!(rendered_unit(release, &target), None, "{target}");
    }
}

#[test]
fn a_home_that_needs_quoting_in_exec_is_refused() {
    assert_eq!(
        command_directory(Path::new("/home/person")).expect("plain home"),
        COMMANDS
    );
    for home in [
        "/home/two words",
        "/home/per%cent",
        "/home/quo\"te",
        "relative/home",
    ] {
        assert!(command_directory(Path::new(home)).is_err(), "{home}");
    }
}
