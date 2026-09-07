//! Emit the WebSocket protocol manifest from the topic registry.
//!
//! `--check` compares the committed file against what the registry
//! would produce and fails on drift, which is the CI gate that makes
//! hand-editing the generated sections pointless.

use std::path::PathBuf;

use anyhow::Context as _;

fn main() -> anyhow::Result<()> {
    let generated = hypercolor_daemon::api::ws::manifest::build_json()
        .context("build the WebSocket protocol manifest")?;
    let path = manifest_path();

    if std::env::args().any(|arg| arg == "--check") {
        let current =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        anyhow::ensure!(
            current == generated,
            "{} is out of date; run `just ws-manifest`",
            path.display()
        );
        return Ok(());
    }

    std::fs::write(&path, generated).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn manifest_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("protocol")
        .join("websocket-v1.json")
}
