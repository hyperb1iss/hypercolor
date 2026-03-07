use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let builtin_root = manifest_dir.join("../../data/attachments/builtin");
    let out_path =
        PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("embedded_attachments.rs");

    let mut files = Vec::new();
    if builtin_root.exists() {
        collect_toml_files(&builtin_root, &mut files);
    }
    files.sort();

    let mut generated =
        String::from("pub(super) const EMBEDDED_ATTACHMENT_TEMPLATES: &[(&str, &str)] = &[\n");

    for path in &files {
        let relative = path
            .strip_prefix(&builtin_root)
            .expect("embedded attachment should live under builtin root");
        let relative_literal = relative.to_string_lossy().replace('\\', "/");
        let path_literal = path
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        writeln!(
            generated,
            "    ({relative_literal:?}, include_str!(\"{path_literal}\")),"
        )
        .expect("write embedded attachment row");
        println!("cargo:rerun-if-changed={}", path.display());
    }

    generated.push_str("];\n");
    fs::write(&out_path, generated).expect("write embedded attachments");
    println!("cargo:rerun-if-changed={}", builtin_root.display());
}

fn collect_toml_files(root: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(root).expect("read attachment directory");

    for entry in entries {
        let entry = entry.expect("read attachment directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_toml_files(&path, out);
            continue;
        }

        let is_toml = path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("toml"));
        if is_toml {
            out.push(path);
        }
    }
}
