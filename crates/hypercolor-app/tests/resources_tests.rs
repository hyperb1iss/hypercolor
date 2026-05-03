use std::{
    fs,
    path::{Path, PathBuf},
};

use hypercolor_app::resources::{
    bundled_effects_resource_dir, install_bundled_effects, install_bundled_runtime_assets,
};

#[test]
fn bundled_effects_resource_dir_matches_tauri_resource_layout() {
    let root = Path::new("/opt/hypercolor/resources");

    assert_eq!(
        bundled_effects_resource_dir(root),
        root.join("effects").join("bundled")
    );
}

#[test]
fn install_bundled_runtime_assets_skips_missing_resource_dir() {
    let report = install_bundled_runtime_assets(None).expect("missing resource dir should skip");

    assert_eq!(report, None);
}

#[test]
fn install_bundled_effects_copies_nested_files() {
    let root = temp_resource_dir("nested-copy");
    let source = root.join("source");
    let destination = root.join("destination");
    let effect = source.join("color-wave.html");
    let nested = source.join("faces").join("clock.html");
    touch(&effect, "<html>effect</html>");
    touch(&nested, "<html>face</html>");

    let report =
        install_bundled_effects(&source, &destination).expect("bundled effects should copy");

    assert_eq!(report.copied_files, 2);
    assert_eq!(
        fs::read_to_string(destination.join("color-wave.html")).expect("effect should be readable"),
        "<html>effect</html>"
    );
    assert_eq!(
        fs::read_to_string(destination.join("faces").join("clock.html"))
            .expect("nested effect should be readable"),
        "<html>face</html>"
    );

    cleanup_temp_resource_dir(&root);
}

fn temp_resource_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "hypercolor-app-resources-{name}-{}",
        std::process::id()
    ));
    cleanup_temp_resource_dir(&dir);
    fs::create_dir_all(&dir).expect("temp resource dir should be created");
    dir
}

fn touch(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent directory should be created");
    }
    fs::write(path, content).expect("test file should be written");
}

fn cleanup_temp_resource_dir(dir: &Path) {
    let temp = std::env::temp_dir();
    if dir.starts_with(&temp) && dir.exists() {
        fs::remove_dir_all(dir).expect("temp resource dir should be removable");
    }
}
