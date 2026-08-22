use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use hypercolor_macos_owner::MacosDaemonOwner;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    identity::hex_bytes,
    model::{
        MAX_REQUEST_BYTES, MAX_WITNESS_EVIDENCE_BYTES, MacosTccCanaryRequest, REQUEST_FILE_NAME,
        is_sha256,
    },
    receipts::MacosTccCanaryWitness,
};

pub fn macos_tcc_canary_directory(data_dir: &Path) -> PathBuf {
    data_dir.join("macos-tcc-canary")
}

pub fn macos_tcc_canary_request_path(data_dir: &Path) -> PathBuf {
    macos_tcc_canary_directory(data_dir).join(REQUEST_FILE_NAME)
}

pub fn validate_macos_tcc_canary_request(request_path: &Path) -> Result<()> {
    read_json_bounded::<MacosTccCanaryRequest>(request_path, MAX_REQUEST_BYTES)?.validate()
}

pub fn publish_macos_tcc_canary_artifact(
    canary_root: &Path,
    source: &Path,
    destination: &Path,
) -> Result<()> {
    ensure_real_directory(canary_root, false)?;
    let parent = destination
        .parent()
        .context("macOS TCC canary artifact destination has no parent")?;
    ensure_canary_descendant_directory(canary_root, parent)?;
    let file_name = destination
        .file_name()
        .context("macOS TCC canary artifact destination has no filename")?;
    anyhow::ensure!(
        matches!(file_name.to_str(), Some(name) if !name.is_empty() && name != "." && name != ".."),
        "macOS TCC canary artifact destination has an invalid filename"
    );
    let (file, metadata) = open_regular_file(source)?;
    anyhow::ensure!(
        metadata.len() <= MAX_WITNESS_EVIDENCE_BYTES,
        "macOS TCC canary artifact exceeds {MAX_WITNESS_EVIDENCE_BYTES} bytes"
    );
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(MAX_WITNESS_EVIDENCE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {}", source.display()))?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_WITNESS_EVIDENCE_BYTES,
        "macOS TCC canary artifact exceeds {MAX_WITNESS_EVIDENCE_BYTES} bytes"
    );
    write_bytes_new(destination, &bytes)
}

pub fn arm_macos_tcc_canary(data_dir: &Path, request_path: &Path) -> Result<PathBuf> {
    let request = read_json_bounded::<MacosTccCanaryRequest>(request_path, MAX_REQUEST_BYTES)?;
    request.validate()?;
    let canary_dir = macos_tcc_canary_directory(data_dir);
    ensure_real_directory(data_dir, false)?;
    ensure_real_directory(&canary_dir, true)?;
    ensure_existing_real_directory(&canary_dir.join("requests"))?;
    ensure_existing_real_directory(&canary_dir.join("receipts"))?;
    fs::set_permissions(&canary_dir, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to secure {}", canary_dir.display()))?;
    let destination = macos_tcc_canary_request_path(data_dir);
    write_json_new(&destination, &request)?;
    sync_parent(&canary_dir)?;
    Ok(destination)
}

#[cfg(feature = "screen-capture")]
pub(super) fn claim_request(
    data_dir: &Path,
    actual_topology: MacosDaemonOwner,
) -> Result<Option<(MacosTccCanaryRequest, PathBuf)>> {
    ensure_real_directory(data_dir, false)?;
    ensure_real_directory(&macos_tcc_canary_directory(data_dir), false)?;
    ensure_existing_real_directory(&macos_tcc_canary_directory(data_dir).join("requests"))?;
    ensure_existing_real_directory(&macos_tcc_canary_directory(data_dir).join("receipts"))?;
    let request_path = macos_tcc_canary_request_path(data_dir);
    if !request_path.exists() {
        return Ok(None);
    }
    let request = read_json_bounded::<MacosTccCanaryRequest>(&request_path, MAX_REQUEST_BYTES)?;
    request.validate()?;
    if request.expected_topology != actual_topology {
        return Ok(None);
    }
    let archive_dir = macos_tcc_canary_directory(data_dir)
        .join("requests")
        .join(&request.run_id);
    ensure_canary_descendant_directory(&macos_tcc_canary_directory(data_dir), &archive_dir)?;
    let archived = archive_dir.join(format!("{}.json", request.row_id));
    anyhow::ensure!(
        !archived.exists(),
        "macOS TCC canary row {} is already archived",
        request.row_id
    );
    fs::rename(&request_path, &archived).with_context(|| {
        format!(
            "failed to claim macOS TCC canary request {}",
            request_path.display()
        )
    })?;
    sync_parent(&macos_tcc_canary_directory(data_dir))?;
    sync_parent(&archive_dir)?;
    Ok(Some((request, archived)))
}

pub(super) fn open_regular_file(path: &Path) -> Result<(File, fs::Metadata)> {
    let path_metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    anyhow::ensure!(
        path_metadata.file_type().is_file() && !path_metadata.file_type().is_symlink(),
        "{} must be a regular non-symlink file",
        path.display()
    );
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let file_metadata = file
        .metadata()
        .with_context(|| format!("failed to inspect opened file {}", path.display()))?;
    anyhow::ensure!(
        file_metadata.file_type().is_file()
            && path_metadata.dev() == file_metadata.dev()
            && path_metadata.ino() == file_metadata.ino(),
        "{} changed while it was being opened",
        path.display()
    );
    Ok((file, file_metadata))
}

pub(super) fn witness_evidence_matches(
    receipt_dir: &Path,
    witness: &MacosTccCanaryWitness,
) -> Result<bool> {
    anyhow::ensure!(
        is_sha256(&witness.evidence_sha256),
        "witness evidence hash is not lowercase SHA-256"
    );
    let path = receipt_dir
        .join("evidence")
        .join(format!("{}.bin", witness.evidence_sha256));
    let (mut file, metadata) = open_regular_file(&path)?;
    anyhow::ensure!(
        metadata.len() <= MAX_WITNESS_EVIDENCE_BYTES,
        "witness evidence exceeds {MAX_WITNESS_EVIDENCE_BYTES} bytes"
    );
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut remaining = MAX_WITNESS_EVIDENCE_BYTES.saturating_add(1);
    while remaining > 0 {
        let read_limit = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded evidence buffer length fits usize");
        let read = file
            .read(&mut buffer[..read_limit])
            .with_context(|| format!("failed to read witness evidence {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        remaining = remaining.saturating_sub(u64::try_from(read).unwrap_or(u64::MAX));
    }
    anyhow::ensure!(remaining > 0, "witness evidence exceeds the read bound");
    Ok(hex_bytes(&hasher.finalize()) == witness.evidence_sha256)
}

pub(super) fn read_json_bounded<T>(path: &Path, maximum: u64) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let (file, metadata) = open_regular_file(path)?;
    anyhow::ensure!(metadata.len() <= maximum, "{} is too large", path.display());
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {}", path.display()))?;
    anyhow::ensure!(
        bytes.len() as u64 <= maximum,
        "{} is too large",
        path.display()
    );
    serde_json::from_slice(&bytes).with_context(|| format!("failed to parse {}", path.display()))
}

pub(super) fn ensure_real_directory(path: &Path, create: bool) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => anyhow::ensure!(
            metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
            "macOS TCC canary directory must be a real directory: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
            fs::create_dir(path).with_context(|| format!("failed to create {}", path.display()))?;
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    }
    Ok(())
}

pub(super) fn ensure_existing_real_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => anyhow::ensure!(
            metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
            "macOS TCC canary directory must be a real directory: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    }
    Ok(())
}

pub(super) fn ensure_canary_descendant_directory(root: &Path, directory: &Path) -> Result<()> {
    ensure_real_directory(root, false)?;
    let relative = directory.strip_prefix(root).with_context(|| {
        format!(
            "macOS TCC canary directory {} escapes {}",
            directory.display(),
            root.display()
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            anyhow::bail!(
                "macOS TCC canary descendant contains traversal: {}",
                directory.display()
            );
        };
        current.push(component);
        ensure_real_directory(&current, true)?;
    }
    Ok(())
}

pub(super) fn write_json_new<T>(path: &Path, value: &T) -> Result<()>
where
    T: Serialize + ?Sized,
{
    let bytes =
        serde_json::to_vec_pretty(value).context("failed to encode macOS TCC canary JSON")?;
    write_bytes_new(path, &[bytes.as_slice(), b"\n"].concat())
}

pub(super) fn write_bytes_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("macOS TCC canary JSON path has no parent")?;
    ensure_real_directory(parent, false)?;
    anyhow::ensure!(!path.exists(), "refusing to overwrite {}", path.display());
    let mut temporary = tempfile::Builder::new()
        .prefix(".macos-tcc-canary-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .with_context(|| format!("failed to create temporary file in {}", parent.display()))?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .with_context(|| format!("failed to write temporary JSON for {}", path.display()))?;
    anyhow::ensure!(!path.exists(), "refusing to overwrite {}", path.display());
    temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to atomically publish {}", path.display()))?;
    sync_parent(parent)
}

pub(super) fn sync_parent(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("failed to sync {}", path.display()))
}
