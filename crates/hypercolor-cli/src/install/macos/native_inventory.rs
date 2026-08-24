use std::path::Path;

use super::super::InstallPlatformError;
use super::model::{MAX_LEGACY_DEPTH, MAX_LEGACY_MEMBERS, error};
use super::public_tree::MacosPublicTree;

pub(super) fn bind_live_inventory(
    tree: &mut MacosPublicTree,
    home: &Path,
    install_prefix: &Path,
    install_dir: &Path,
) -> Result<(), InstallPlatformError> {
    let named = [
        install_dir.join("hyper"),
        install_dir.join("hypercolor-tray"),
        install_prefix.join("share/bash-completion/completions/hyper"),
        install_prefix.join("share/zsh/site-functions/_hyper"),
        home.join(".config/fish/completions/hyper.fish"),
    ]
    .into_iter()
    .map(exact_path)
    .collect::<Result<Vec<_>, _>>()?;
    tree.bind_paths(std::iter::empty(), named)?;
    let (data_directories, data_entries, data_members) = tree.discover_tree(
        &install_prefix.join("share/hypercolor"),
        true,
        MAX_LEGACY_DEPTH,
        MAX_LEGACY_MEMBERS,
    )?;
    let remaining = MAX_LEGACY_MEMBERS
        .checked_sub(data_members)
        .ok_or_else(|| error("macOS legacy public inventory exceeds its member bound"))?;
    let (icon_directories, icon_entries, _) = tree.discover_tree(
        &install_prefix.join("share/icons"),
        false,
        MAX_LEGACY_DEPTH,
        remaining,
    )?;
    tree.bind_paths(
        data_directories.into_iter().chain(icon_directories),
        data_entries.into_iter().chain(icon_entries),
    )
}

fn exact_path(path: std::path::PathBuf) -> Result<String, InstallPlatformError> {
    let path = path
        .to_str()
        .ok_or_else(|| error("macOS historical public path is not exact UTF-8"))?
        .to_owned();
    super::model::validate_public_path(&path)?;
    Ok(path)
}
