//! Prepared media replacements keep content staging separate from publication.

mod intent;

use std::collections::BTreeMap;
use std::io::{BufReader, Read, Seek, SeekFrom};

use hypercolor_persistence::{AtomicWriteReservation, AtomicWriteWaitOutcome, PersistenceShutdown};
use hypercolor_types::portable_setup::{PortableAsset, PortableBlob};
use tempfile::NamedTempFile;
use uuid::Uuid;

use super::*;
use crate::portable_setup::SetupBlobStore;

/// A validated replacement. The caller must retain the library's mutation
/// guard and durably record its transaction before calling `persist`.
pub struct PreparedAssetReplacement {
    candidate: AssetLibrary,
    objects: Vec<StagedObject>,
    retired: Vec<(PathBuf, PathBuf)>,
    reservation: Option<AtomicWriteReservation>,
    payload: Vec<u8>,
    durable: bool,
    intent: intent::MediaIntent,
    intent_durable: bool,
}

struct StagedObject {
    temporary: NamedTempFile,
    destination: PathBuf,
    published: bool,
}

impl AssetLibrary {
    /// Authored metadata only; scan results and timestamps remain local.
    #[must_use]
    pub fn portable_snapshot(&self) -> BTreeMap<Uuid, PortableAsset> {
        self.records()
            .iter()
            .map(|record| {
                (
                    record.id.0,
                    PortableAsset {
                        name: record.name.clone(),
                        mime_type: record.mime_type.clone(),
                        tags: record.tags.clone(),
                        content: PortableBlob {
                            sha256: record.hash_sha256.clone(),
                            byte_len: record.byte_len,
                        },
                    },
                )
            })
            .collect()
    }

    /// Validate and stage a complete authored replacement without modifying
    /// the index or admitting canonical object writes.
    pub fn prepare_portable_replace(
        &self,
        expected: &BTreeMap<Uuid, PortableAsset>,
        desired: &BTreeMap<Uuid, PortableAsset>,
        blobs: &SetupBlobStore,
        operation_id: Uuid,
    ) -> anyhow::Result<PreparedAssetReplacement> {
        self.prepare_portable_impl(expected, desired, blobs, operation_id, false)
    }

    /// Resume only the operation proven by the durable media intent and the
    /// coordinator's original journal. Ordinary replacements retain strict CAS.
    pub fn prepare_portable_recovery(
        &self,
        expected: &BTreeMap<Uuid, PortableAsset>,
        desired: &BTreeMap<Uuid, PortableAsset>,
        blobs: &SetupBlobStore,
        operation_id: Uuid,
    ) -> anyhow::Result<PreparedAssetReplacement> {
        let previous = intent::read(&self.root)?;
        if previous.as_ref().is_none_or(|intent| intent.completed) {
            // The global journal can be durable before this participant has
            // admitted its own intent. Only the exact before image is safe.
            return self.prepare_portable_impl(expected, desired, blobs, operation_id, false);
        }
        let intent = previous.expect("pending intent checked above");
        anyhow::ensure!(
            !intent.completed && intent.matches(operation_id, expected, desired),
            "media recovery intent differs from global journal"
        );
        let current = self.portable_snapshot();
        for (id, value) in &current {
            anyhow::ensure!(
                expected.get(id) == Some(value) || desired.get(id) == Some(value),
                "media changed outside pending restore"
            );
        }
        let already_desired = current == *desired;
        for (id, old) in expected {
            if !already_desired && !current.contains_key(id) {
                let source = self.object_path_for_hash(&old.content.sha256)?;
                let retired = source.with_extension(format!("retired-{operation_id}"));
                let proof = blobs.capture(&retired)?;
                anyhow::ensure!(
                    proof == old.content,
                    "missing media lacks exact operation retirement proof"
                );
            }
        }
        self.prepare_portable_impl(expected, desired, blobs, operation_id, true)
    }

    fn prepare_portable_impl(
        &self,
        expected: &BTreeMap<Uuid, PortableAsset>,
        desired: &BTreeMap<Uuid, PortableAsset>,
        blobs: &SetupBlobStore,
        operation_id: Uuid,
        recovering: bool,
    ) -> anyhow::Result<PreparedAssetReplacement> {
        anyhow::ensure!(
            recovering || self.portable_snapshot() == *expected,
            "media changed during restore preparation"
        );
        let mut candidate = self.clone();
        let mut records = Vec::new();
        let mut objects = Vec::new();
        let mut staged_hashes = HashSet::new();
        for (id, asset) in desired {
            asset.content.validate()?;
            let existing = self.get(AssetId(*id));
            let mut record = if !recovering && expected.get(id) == Some(asset) {
                existing
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("missing media identity"))?
            } else {
                let mut blob = blobs.read(&asset.content)?;
                anyhow::ensure!(
                    asset.content.byte_len <= self.limits.hard_file_cap_bytes,
                    "media exceeds local hard file limit"
                );
                let (scan, thumbnail) = scan_file(
                    &mut blob.file,
                    &asset.mime_type,
                    &self.stream_url_policy,
                    self.limits.thumbnail_size,
                )?;
                anyhow::ensure!(
                    scan.mime_type == asset.mime_type,
                    "media MIME differs from its verified contents"
                );
                if staged_hashes.insert(asset.content.sha256.clone()) {
                    blob.file.rewind()?;
                    let mut temporary = NamedTempFile::new_in(&self.root)?;
                    let mut hasher = Sha256::new();
                    let mut byte_len = 0_u64;
                    let mut buffer = [0_u8; 16 * 1024];
                    loop {
                        let count = blob.file.read(&mut buffer)?;
                        if count == 0 {
                            break;
                        }
                        hasher.update(&buffer[..count]);
                        temporary.as_file_mut().write_all(&buffer[..count])?;
                        byte_len += count as u64;
                    }
                    anyhow::ensure!(
                        byte_len == asset.content.byte_len
                            && digest_hex(&hasher.finalize()) == asset.content.sha256,
                        "media changed while staging"
                    );
                    temporary.as_file().sync_all()?;
                    objects.push(StagedObject {
                        temporary,
                        destination: self.object_path_for_hash(&asset.content.sha256)?,
                        published: false,
                    });
                }
                if let Some(image) = thumbnail {
                    let mut temporary = NamedTempFile::new_in(&self.root)?;
                    image.write_to(temporary.as_file_mut(), ImageFormat::WebP)?;
                    temporary.as_file().sync_all()?;
                    objects.push(StagedObject {
                        temporary,
                        destination: self.thumbnail_path(AssetId(*id)),
                        published: false,
                    });
                }
                let now = now_utc();
                MediaAssetRecord {
                    id: AssetId(*id),
                    name: asset.name.clone(),
                    hash_sha256: asset.content.sha256.clone(),
                    mime_type: scan.mime_type,
                    byte_len: asset.content.byte_len,
                    intrinsic_width: scan.intrinsic_width,
                    intrinsic_height: scan.intrinsic_height,
                    duration_us: scan.duration_us,
                    frame_count: scan.frame_count,
                    tags: asset.tags.clone(),
                    created_at: existing.map_or(now, |entry| entry.created_at),
                    modified_at: now,
                    scan_status: AssetScanStatus::Ready,
                    warnings: self.warnings_for_new_asset(asset.content.byte_len),
                }
            };
            record.name.clone_from(&asset.name);
            record.tags.clone_from(&asset.tags);
            records.push(record);
        }
        candidate.index.replace_records(records);
        let hashes: HashSet<_> = desired
            .values()
            .map(|asset| &asset.content.sha256)
            .collect();
        let mut retired = Vec::new();
        for hash in expected
            .values()
            .map(|asset| &asset.content.sha256)
            .collect::<HashSet<_>>()
        {
            if !hashes.contains(hash) {
                let path = self.object_path_for_hash(hash)?;
                let destination = path.with_extension(format!("retired-{operation_id}"));
                retired.push((path, destination));
            }
        }
        let payload = serialize_json_pretty(&candidate.index)?;
        Ok(PreparedAssetReplacement {
            candidate,
            objects,
            retired,
            reservation: Some(self.index_writer.reserve()),
            payload,
            durable: false,
            intent: intent::MediaIntent::new(&self.root, operation_id, expected, desired)?,
            intent_durable: false,
        })
    }
}

impl PreparedAssetReplacement {
    /// Run on an owned blocking worker after the global restore journal is
    /// durable. File steps are idempotent across recoverable I/O failures.
    pub fn persist(
        &mut self,
        shutdown: &PersistenceShutdown,
    ) -> anyhow::Result<AtomicWriteWaitOutcome> {
        if self.durable {
            return Ok(AtomicWriteWaitOutcome::Written);
        }
        let files = self.persist_files(shutdown)?;
        if files != AtomicWriteWaitOutcome::Written {
            return Ok(files);
        }
        let outcome = self
            .reservation
            .take()
            .ok_or_else(|| anyhow::anyhow!("media index already admitted"))?
            .admit(std::mem::take(&mut self.payload))
            .commit_and_wait_durable_or_shutdown(shutdown);
        self.durable = outcome == AtomicWriteWaitOutcome::Written;
        Ok(outcome)
    }

    fn persist_files(
        &mut self,
        shutdown: &PersistenceShutdown,
    ) -> anyhow::Result<AtomicWriteWaitOutcome> {
        if !self.intent_durable {
            let outcome = self.intent.persist(&self.candidate.root, shutdown)?;
            if outcome != AtomicWriteWaitOutcome::Written {
                return Ok(outcome);
            }
            self.intent_durable = true;
        }
        for object in &mut self.objects {
            if shutdown.is_shutdown() {
                return Ok(AtomicWriteWaitOutcome::ShutdownPending);
            }
            if !object.published {
                let parent = object
                    .destination
                    .parent()
                    .ok_or_else(|| anyhow::anyhow!("media destination has no parent"))?;
                fs::create_dir_all(parent)?;
                // Keep the staged source for a retry after a visible rename
                // whose directory durability barrier failed.
                let mut temporary = NamedTempFile::new_in(parent)?;
                object.temporary.as_file_mut().rewind()?;
                std::io::copy(object.temporary.as_file_mut(), temporary.as_file_mut())?;
                temporary.as_file().sync_all()?;
                hypercolor_platform_fs::durable_replace(temporary.path(), &object.destination)?;
                object.published = true;
            }
        }
        for (source, destination) in &self.retired {
            if shutdown.is_shutdown() {
                return Ok(AtomicWriteWaitOutcome::ShutdownPending);
            }
            retire_file(source, destination)?;
        }
        if shutdown.is_shutdown() {
            return Ok(AtomicWriteWaitOutcome::ShutdownPending);
        }
        Ok(AtomicWriteWaitOutcome::Written)
    }

    /// Publish only after all participants in the restore are durable.
    pub fn publish(self, library: &mut AssetLibrary) {
        assert!(self.durable, "media index durable before publication");
        *library = self.candidate;
    }
}

fn scan_file(
    file: &mut File,
    expected_mime: &str,
    policy: &StreamUrlPolicy,
    thumbnail_size: u32,
) -> anyhow::Result<(ScannedMetadata, Option<image::DynamicImage>)> {
    file.rewind()?;
    let mut prefix = [0_u8; 12];
    let count = file.read(&mut prefix)?;
    let mut mime = sniff_mime(&prefix[..count], None, policy);
    if is_png(&prefix[..count]) && png_is_animated(file)? {
        mime = Some("image/apng".into());
    }
    if expected_mime == "application/json" {
        file.rewind()?;
        // Match the existing Lottie upload hint's JSON validation without a
        // second allocation containing the entire encoded file.
        serde_json::from_reader::<_, serde::de::IgnoredAny>(BufReader::new(&mut *file))?;
        mime = Some("application/json".into());
    } else if expected_mime == "application/vnd.hypercolor.stream-url" {
        file.rewind()?;
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        anyhow::ensure!(
            stream_url_from_bytes_with_policy(text.as_bytes(), policy).is_some(),
            "invalid stream URL"
        );
        mime = Some(expected_mime.into());
    }
    let mime_type =
        mime.ok_or_else(|| anyhow::anyhow!("unsupported or unverifiable media signature"))?;
    let mut scan = ScannedMetadata {
        mime_type,
        intrinsic_width: None,
        intrinsic_height: None,
        duration_us: None,
        frame_count: None,
    };
    let thumbnail = if is_thumbnail_source(&scan.mime_type) {
        file.rewind()?;
        let image = image::ImageReader::new(BufReader::new(file))
            .with_guessed_format()?
            .decode()?;
        let (width, height) = image.dimensions();
        scan.intrinsic_width = Some(width);
        scan.intrinsic_height = Some(height);
        Some(image.thumbnail(thumbnail_size, thumbnail_size))
    } else {
        None
    };
    Ok((scan, thumbnail))
}

fn png_is_animated(file: &mut File) -> std::io::Result<bool> {
    file.seek(SeekFrom::Start(8))?;
    let length = file.metadata()?.len();
    loop {
        let mut header = [0_u8; 8];
        if let Err(error) = file.read_exact(&mut header) {
            if error.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok(false);
            }
            return Err(error);
        }
        let chunk_len = u32::from_be_bytes(header[..4].try_into().expect("four-byte chunk length"));
        let end = file
            .stream_position()?
            .saturating_add(u64::from(chunk_len))
            .saturating_add(4);
        if end > length {
            return Ok(false);
        }
        if &header[4..] == b"acTL" {
            return Ok(true);
        }
        if &header[4..] == b"IDAT" {
            return Ok(false);
        }
        file.seek(SeekFrom::Start(end))?;
    }
}

fn retire_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    if source.exists() {
        hypercolor_platform_fs::durable_replace(source, destination)?;
    } else if destination.exists() {
        // A prior rename may have become visible before its durability
        // barrier failed. Replacing the retired entry repeats that barrier.
        let parent = destination
            .parent()
            .ok_or_else(|| std::io::Error::other("missing retirement parent"))?;
        let mut temporary = NamedTempFile::new_in(parent)?;
        let mut previous = hypercolor_platform_fs::open_no_follow(destination)?;
        std::io::copy(&mut previous, temporary.as_file_mut())?;
        // Windows cannot replace the destination while its data handle is open.
        drop(previous);
        temporary.as_file().sync_all()?;
        hypercolor_platform_fs::durable_replace(temporary.path(), destination)?;
    }
    Ok(())
}

fn digest_hex(digest: &[u8]) -> String {
    let mut result = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut result, "{byte:02x}").expect("String formatting");
    }
    result
}

/// Whether startup must defer object reconciliation to an unfinished restore.
pub(super) fn has_pending_restore(root: &Path) -> anyhow::Result<bool> {
    intent::pending(root)
}

/// Clear pending recovery authority only after the coordinator's committed
/// receipt is durable. The receipt's desired assets must match exactly.
pub fn finalize_portable_restore(
    root: &Path,
    operation_id: Uuid,
    desired: &BTreeMap<Uuid, PortableAsset>,
    shutdown: &PersistenceShutdown,
) -> anyhow::Result<AtomicWriteWaitOutcome> {
    intent::finalize(root, operation_id, desired, shutdown)
}

#[cfg(test)]
mod recovery_tests;
