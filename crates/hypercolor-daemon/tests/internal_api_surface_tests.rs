//! Source fences for the daemon's internal API ownership boundaries.

use std::path::{Path, PathBuf};

fn daemon_sources() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut pending = vec![root.clone()];
    let mut sources = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).expect("daemon source directory should read") {
            let path = entry.expect("daemon source entry should read").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let source = std::fs::read_to_string(&path)
                    .expect("daemon Rust source should read as UTF-8");
                sources.push((path, source));
            }
        }
    }
    sources
}

#[test]
fn app_state_has_one_module_identity() {
    let banned = [
        "crate::api::AppState",
        "hypercolor_daemon::api::AppState",
        "pub use crate::app_state::AppState",
    ];
    let offenders = daemon_sources()
        .into_iter()
        .flat_map(|(path, source)| {
            banned.into_iter().filter_map(move |pattern| {
                source
                    .contains(pattern)
                    .then(|| format!("{} contains {pattern}", path.display()))
            })
        })
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "AppState belongs to app_state, not the transport API:\n{}",
        offenders.join("\n")
    );
}
