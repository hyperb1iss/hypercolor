//! Content-addressed user media asset storage.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use hypercolor_types::asset::AssetId;
use image::{GenericImageView, ImageFormat};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tracing::warn;

use super::index::{
    AssetEvent, AssetIndex, AssetScanStatus, AssetWarning, INDEX_VERSION, MediaAssetRecord,
};

const OBJECTS_DIR: &str = "objects";
const THUMBNAILS_DIR: &str = "thumbnails";
const INDEX_FILE: &str = "index.json";
const INDEX_TMP_FILE: &str = "index.json.tmp";
const THUMBNAIL_SIZE: u32 = 256;
const BYTES_PER_MIB: u64 = 1024 * 1024;
const BYTES_PER_GIB: u64 = 1024 * BYTES_PER_MIB;

/// Default asset library policy limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetLibraryLimits {
    pub per_asset_soft_cap_bytes: u64,
    pub library_soft_cap_bytes: u64,
    pub hard_file_cap_bytes: u64,
    pub thumbnail_size: u32,
}

impl Default for AssetLibraryLimits {
    fn default() -> Self {
        Self {
            per_asset_soft_cap_bytes: 256 * BYTES_PER_MIB,
            library_soft_cap_bytes: 4 * BYTES_PER_GIB,
            hard_file_cap_bytes: 2 * BYTES_PER_GIB,
            thumbnail_size: THUMBNAIL_SIZE,
        }
    }
}

/// Optional media type hints supplied by upload callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetTypeHint {
    Lottie,
    Stream,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamUrlPolicy {
    private_network_allowlist: Vec<StreamIpRule>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamIpRule {
    Exact(IpAddr),
    Cidr { network: IpAddr, prefix: u8 },
}

impl StreamUrlPolicy {
    #[must_use]
    pub fn from_private_network_allowlist<I, S>(rules: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            private_network_allowlist: rules
                .into_iter()
                .filter_map(|rule| StreamIpRule::parse(rule.as_ref().trim()))
                .collect(),
        }
    }

    fn allows_url(&self, raw: &str) -> bool {
        let Ok(url) = reqwest::Url::parse(raw) else {
            return false;
        };
        if !matches!(url.scheme(), "http" | "https" | "rtmp" | "rtsp") {
            return false;
        }
        let Some(host) = url.host_str() else {
            return false;
        };
        if is_local_hostname(host) {
            return false;
        }
        if let Some(ip) = host_as_ip(host) {
            let ip = canonical_ip(ip);
            return is_public_ip(ip) || self.allows_private_ip(ip);
        }
        let port = match url.scheme() {
            "rtsp" => url.port().unwrap_or(554),
            "rtmp" => url.port().unwrap_or(1935),
            _ => url.port_or_known_default().unwrap_or(443),
        };
        let Ok(resolved) = (host, port).to_socket_addrs() else {
            return false;
        };
        let mut resolved_any = false;
        for address in resolved {
            resolved_any = true;
            let ip = canonical_ip(address.ip());
            if !(is_public_ip(ip) || self.allows_private_ip(ip)) {
                return false;
            }
        }
        resolved_any
    }

    fn allows_private_ip(&self, ip: IpAddr) -> bool {
        self.private_network_allowlist
            .iter()
            .any(|rule| rule.matches(ip))
    }
}

impl StreamIpRule {
    fn parse(raw: &str) -> Option<Self> {
        if raw.is_empty() {
            return None;
        }
        if let Some((network, prefix)) = raw.split_once('/') {
            let network = network.parse::<IpAddr>().ok()?;
            let prefix = prefix.parse::<u8>().ok()?;
            return cidr_prefix_valid(network, prefix).then_some(Self::Cidr { network, prefix });
        }
        raw.parse::<IpAddr>().ok().map(canonical_ip).map(Self::Exact)
    }

    fn matches(self, ip: IpAddr) -> bool {
        match self {
            Self::Exact(rule_ip) => rule_ip == ip,
            Self::Cidr { network, prefix } => cidr_contains(network, prefix, ip),
        }
    }
}

/// Upload options for adding a media asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetUploadOptions {
    pub name: String,
    pub tags: Vec<String>,
    pub type_hint: Option<AssetTypeHint>,
    pub rename_duplicate: bool,
}

impl AssetUploadOptions {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tags: Vec::new(),
            type_hint: None,
            rename_duplicate: false,
        }
    }
}

/// Result of an asset upload.
#[derive(Debug, Clone, PartialEq)]
pub struct AssetUpsert {
    pub record: MediaAssetRecord,
    pub duplicate: bool,
    pub events: Vec<AssetEvent>,
}

/// Result of updating mutable asset metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct AssetMetadataUpdate {
    pub record: MediaAssetRecord,
    pub event: Option<AssetEvent>,
}

/// Errors produced by the asset library.
#[derive(Debug, thiserror::Error)]
pub enum AssetLibraryError {
    #[error("asset file exceeds hard cap: {byte_len} bytes > {hard_cap_bytes} bytes")]
    HardCapExceeded { byte_len: u64, hard_cap_bytes: u64 },
    #[error("unsupported asset media type: {reason}")]
    UnsupportedMediaType { reason: String },
    #[error("asset not found: {0}")]
    NotFound(AssetId),
    #[error("invalid asset hash path: {path}")]
    InvalidHashPath { path: PathBuf },
    #[error("failed to create asset directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read asset file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write asset file {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to replace asset file {path}: {source}")]
    Replace {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to sync asset file {path}: {source}")]
    Sync {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse asset index {path}: {source}")]
    ParseIndex {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to serialize asset index: {0}")]
    SerializeIndex(#[source] serde_json::Error),
    #[error("failed to decode asset image: {0}")]
    DecodeImage(#[source] image::ImageError),
    #[error("failed to encode asset thumbnail {path}: {source}")]
    EncodeThumbnail {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },
}

/// Content-addressed media asset library rooted at `assets/`.
#[derive(Debug, Clone)]
pub struct AssetLibrary {
    root: PathBuf,
    objects_dir: PathBuf,
    thumbnails_dir: PathBuf,
    index_path: PathBuf,
    index: AssetIndex,
    limits: AssetLibraryLimits,
    stream_url_policy: StreamUrlPolicy,
}

impl AssetLibrary {
    /// Open an asset library at `root`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, AssetLibraryError> {
        Self::open_with_limits(root, AssetLibraryLimits::default())
    }

    pub fn open_with_stream_url_policy(
        root: impl Into<PathBuf>,
        stream_url_policy: StreamUrlPolicy,
    ) -> Result<Self, AssetLibraryError> {
        Self::open_with_limits_and_stream_url_policy(
            root,
            AssetLibraryLimits::default(),
            stream_url_policy,
        )
    }

    /// Open an asset library at `root` with explicit policy limits.
    pub fn open_with_limits(
        root: impl Into<PathBuf>,
        limits: AssetLibraryLimits,
    ) -> Result<Self, AssetLibraryError> {
        Self::open_with_limits_and_stream_url_policy(root, limits, StreamUrlPolicy::default())
    }

    pub fn open_with_limits_and_stream_url_policy(
        root: impl Into<PathBuf>,
        limits: AssetLibraryLimits,
        stream_url_policy: StreamUrlPolicy,
    ) -> Result<Self, AssetLibraryError> {
        let root = root.into();
        let objects_dir = root.join(OBJECTS_DIR);
        let thumbnails_dir = root.join(THUMBNAILS_DIR);
        let index_path = root.join(INDEX_FILE);

        create_dir(&root)?;
        create_dir(&objects_dir)?;
        create_dir(&thumbnails_dir)?;
        set_private_dir_permissions(&root);

        let index = load_index(&index_path)?.unwrap_or_default();

        let mut library = Self {
            root,
            objects_dir,
            thumbnails_dir,
            index_path,
            index,
            limits,
            stream_url_policy,
        };

        if !library.index_path.exists() || library.index.records().is_empty() {
            library.rebuild_index_from_objects()?;
        }

        Ok(library)
    }

    #[must_use]
    pub fn stream_url_policy(&self) -> &StreamUrlPolicy {
        &self.stream_url_policy
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn index_path(&self) -> &Path {
        &self.index_path
    }

    #[must_use]
    pub fn records(&self) -> &[MediaAssetRecord] {
        self.index.records()
    }

    #[must_use]
    pub fn get(&self, id: AssetId) -> Option<&MediaAssetRecord> {
        self.index.get(id)
    }

    #[must_use]
    pub fn contains(&self, id: AssetId) -> bool {
        self.get(id).is_some()
    }

    pub fn add_file(
        &mut self,
        path: &Path,
        options: AssetUploadOptions,
    ) -> Result<AssetUpsert, AssetLibraryError> {
        let byte_len = fs::metadata(path)
            .map_err(|source| AssetLibraryError::Read {
                path: path.to_path_buf(),
                source,
            })?
            .len();
        if byte_len > self.limits.hard_file_cap_bytes {
            return Err(AssetLibraryError::HardCapExceeded {
                byte_len,
                hard_cap_bytes: self.limits.hard_file_cap_bytes,
            });
        }
        let bytes = fs::read(path).map_err(|source| AssetLibraryError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        self.add_bytes(&bytes, options)
    }

    pub fn add_bytes(
        &mut self,
        bytes: &[u8],
        options: AssetUploadOptions,
    ) -> Result<AssetUpsert, AssetLibraryError> {
        let byte_len = bytes_len(bytes);
        if byte_len > self.limits.hard_file_cap_bytes {
            return Err(AssetLibraryError::HardCapExceeded {
                byte_len,
                hard_cap_bytes: self.limits.hard_file_cap_bytes,
            });
        }

        let hash = Self::hash_bytes(bytes);
        if let Some(existing) = self.index.by_hash(&hash).cloned() {
            return self.handle_duplicate(existing, options);
        }

        let scan = scan_metadata(bytes, options.type_hint, &self.stream_url_policy)?;
        let object_path = self.write_object_once(&hash, bytes)?;
        let now = now_utc();
        let record = MediaAssetRecord {
            id: AssetId::new(),
            name: sanitize_name(&options.name),
            hash_sha256: hash,
            mime_type: scan.mime_type,
            byte_len,
            intrinsic_width: scan.intrinsic_width,
            intrinsic_height: scan.intrinsic_height,
            duration_us: scan.duration_us,
            frame_count: scan.frame_count,
            tags: sanitize_tags(options.tags),
            created_at: now,
            modified_at: now,
            scan_status: AssetScanStatus::Ready,
            warnings: self.warnings_for_new_asset(byte_len),
        };

        if is_thumbnail_source(&record.mime_type) {
            self.write_thumbnail(&record.id, bytes)?;
        }

        self.index.upsert(record.clone());
        self.persist_index()?;

        let event = AssetEvent::Added {
            record: record.clone(),
        };
        warn_if_over_soft_cap(&record, &object_path, self.limits);

        Ok(AssetUpsert {
            record,
            duplicate: false,
            events: vec![event],
        })
    }

    pub fn remove(&mut self, id: AssetId) -> Result<Option<AssetEvent>, AssetLibraryError> {
        let Some(record) = self.index.remove(id) else {
            return Ok(None);
        };

        let object_path = self.object_path_for_hash(&record.hash_sha256)?;
        let _ = fs::remove_file(object_path);
        let _ = fs::remove_file(self.thumbnail_path(record.id));
        self.persist_index()?;
        Ok(Some(AssetEvent::Removed { asset_id: id }))
    }

    pub fn update_metadata(
        &mut self,
        id: AssetId,
        name: Option<String>,
        tags: Option<Vec<String>>,
    ) -> Result<Option<AssetMetadataUpdate>, AssetLibraryError> {
        let Some(previous) = self.index.get(id).cloned() else {
            return Ok(None);
        };
        let mut record = previous.clone();
        if let Some(name) = name {
            record.name = sanitize_name(&name);
        }
        if let Some(tags) = tags {
            record.tags = sanitize_tags(tags);
        }
        if record == previous {
            return Ok(Some(AssetMetadataUpdate {
                record,
                event: None,
            }));
        }

        record.modified_at = now_utc();
        self.index.upsert(record.clone());
        self.persist_index()?;
        Ok(Some(AssetMetadataUpdate {
            record: record.clone(),
            event: Some(AssetEvent::Modified { record }),
        }))
    }

    pub fn rebuild_index_from_objects(&mut self) -> Result<Vec<AssetEvent>, AssetLibraryError> {
        self.reconcile_from_disk(false)
    }

    pub fn refresh_from_disk(&mut self) -> Result<Vec<AssetEvent>, AssetLibraryError> {
        let previous_index = self.index.clone();
        let mut events = Vec::new();
        if let Some(index) = load_index(&self.index_path)? {
            self.index = index;
            events.extend(index_change_events(&previous_index, &self.index));
        }
        events.extend(self.reconcile_from_disk(true)?);
        Ok(events)
    }

    #[must_use]
    pub fn thumbnail_path(&self, id: AssetId) -> PathBuf {
        self.thumbnails_dir.join(format!("{id}.webp"))
    }

    pub fn object_path_for_hash(&self, hash_sha256: &str) -> Result<PathBuf, AssetLibraryError> {
        if !is_sha256_hex(hash_sha256) {
            return Err(AssetLibraryError::InvalidHashPath {
                path: PathBuf::from(hash_sha256),
            });
        }
        Ok(self
            .objects_dir
            .join(&hash_sha256[0..2])
            .join(&hash_sha256[2..]))
    }

    #[must_use]
    pub fn hash_bytes(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        let mut hash = String::with_capacity(64);
        for byte in digest {
            write!(&mut hash, "{byte:02x}").expect("writing to String cannot fail");
        }
        hash
    }

    fn handle_duplicate(
        &mut self,
        mut record: MediaAssetRecord,
        options: AssetUploadOptions,
    ) -> Result<AssetUpsert, AssetLibraryError> {
        if !options.rename_duplicate {
            return Ok(AssetUpsert {
                record,
                duplicate: true,
                events: Vec::new(),
            });
        }

        record.name = sanitize_name(&options.name);
        record.tags = sanitize_tags(options.tags);
        record.modified_at = now_utc();
        self.index.upsert(record.clone());
        self.persist_index()?;
        Ok(AssetUpsert {
            record: record.clone(),
            duplicate: true,
            events: vec![AssetEvent::Modified { record }],
        })
    }

    fn reconcile_from_disk(
        &mut self,
        preserve_index_records: bool,
    ) -> Result<Vec<AssetEvent>, AssetLibraryError> {
        let previous_by_id: HashMap<AssetId, MediaAssetRecord> = self
            .index
            .records()
            .iter()
            .cloned()
            .map(|record| (record.id, record))
            .collect();
        let previous_by_hash: HashMap<String, MediaAssetRecord> = previous_by_id
            .values()
            .cloned()
            .map(|record| (record.hash_sha256.clone(), record))
            .collect();

        let discovered = self.discover_objects()?;
        let mut seen_path_hashes = HashSet::new();
        let mut records = Vec::new();
        let mut events = Vec::new();

        for discovered_object in discovered {
            seen_path_hashes.insert(discovered_object.path_hash.clone());

            if discovered_object.actual_hash == discovered_object.path_hash {
                if let Some(record) = previous_by_hash.get(&discovered_object.actual_hash) {
                    records.push(record.clone());
                } else {
                    let record = self.unscanned_record(
                        discovered_object.actual_hash,
                        discovered_object.byte_len,
                    );
                    events.push(AssetEvent::Added {
                        record: record.clone(),
                    });
                    records.push(record);
                }
                continue;
            }

            self.write_object_once(&discovered_object.actual_hash, &discovered_object.bytes)?;
            if let Some(record) = previous_by_hash.get(&discovered_object.actual_hash) {
                records.push(record.clone());
            } else {
                let record = self
                    .unscanned_record(discovered_object.actual_hash, discovered_object.byte_len);
                events.push(AssetEvent::Added {
                    record: record.clone(),
                });
                records.push(record);
            }
        }

        if preserve_index_records {
            for record in previous_by_id.values() {
                if seen_path_hashes.contains(&record.hash_sha256)
                    && !records.iter().any(|candidate| candidate.id == record.id)
                {
                    records.push(record.clone());
                }
            }
        }

        let next_ids: HashSet<AssetId> = records.iter().map(|record| record.id).collect();
        for id in previous_by_id.keys().copied() {
            if !next_ids.contains(&id) {
                events.push(AssetEvent::Removed { asset_id: id });
            }
        }

        self.index.replace_records(records);
        self.persist_index()?;
        Ok(events)
    }

    fn discover_objects(&self) -> Result<Vec<DiscoveredObject>, AssetLibraryError> {
        let mut objects = Vec::new();
        if !self.objects_dir.exists() {
            return Ok(objects);
        }

        for shard in fs::read_dir(&self.objects_dir).map_err(|source| AssetLibraryError::Read {
            path: self.objects_dir.clone(),
            source,
        })? {
            let shard = shard.map_err(|source| AssetLibraryError::Read {
                path: self.objects_dir.clone(),
                source,
            })?;
            let shard_path = shard.path();
            if !shard_path.is_dir() {
                continue;
            }
            let Some(prefix) = shard_path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            for entry in fs::read_dir(&shard_path).map_err(|source| AssetLibraryError::Read {
                path: shard_path.clone(),
                source,
            })? {
                let entry = entry.map_err(|source| AssetLibraryError::Read {
                    path: shard_path.clone(),
                    source,
                })?;
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let Some(suffix) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                let path_hash = format!("{prefix}{suffix}");
                if !is_sha256_hex(&path_hash) {
                    continue;
                }
                let bytes = fs::read(&path).map_err(|source| AssetLibraryError::Read {
                    path: path.clone(),
                    source,
                })?;
                let actual_hash = Self::hash_bytes(&bytes);
                objects.push(DiscoveredObject {
                    path_hash,
                    actual_hash,
                    byte_len: bytes_len(&bytes),
                    bytes,
                });
            }
        }

        objects.sort_by(|left, right| left.path_hash.cmp(&right.path_hash));
        Ok(objects)
    }

    fn unscanned_record(&self, hash_sha256: String, byte_len: u64) -> MediaAssetRecord {
        let now = now_utc();
        MediaAssetRecord {
            id: AssetId::new(),
            name: format!("asset-{hash_sha256:.12}"),
            hash_sha256,
            mime_type: "application/octet-stream".to_owned(),
            byte_len,
            intrinsic_width: None,
            intrinsic_height: None,
            duration_us: None,
            frame_count: None,
            tags: Vec::new(),
            created_at: now,
            modified_at: now,
            scan_status: AssetScanStatus::Unscanned,
            warnings: self.warnings_for_new_asset(byte_len),
        }
    }

    fn write_object_once(
        &self,
        hash_sha256: &str,
        bytes: &[u8],
    ) -> Result<PathBuf, AssetLibraryError> {
        let object_path = self.object_path_for_hash(hash_sha256)?;
        if object_path.exists() {
            return Ok(object_path);
        }

        let Some(parent) = object_path.parent() else {
            return Err(AssetLibraryError::InvalidHashPath { path: object_path });
        };
        create_dir(parent)?;

        let tmp_path = parent.join(format!("{hash_sha256}.tmp"));
        write_file_synced(&tmp_path, bytes)?;
        fs::rename(&tmp_path, &object_path).map_err(|source| AssetLibraryError::Replace {
            path: object_path.clone(),
            source,
        })?;
        sync_dir(parent)?;
        Ok(object_path)
    }

    fn write_thumbnail(&self, id: &AssetId, bytes: &[u8]) -> Result<(), AssetLibraryError> {
        let image = image::load_from_memory(bytes).map_err(AssetLibraryError::DecodeImage)?;
        let thumbnail = image.thumbnail(self.limits.thumbnail_size, self.limits.thumbnail_size);
        let path = self.thumbnail_path(*id);
        thumbnail
            .save_with_format(&path, ImageFormat::WebP)
            .map_err(|source| AssetLibraryError::EncodeThumbnail {
                path: path.clone(),
                source,
            })?;
        Ok(())
    }

    fn persist_index(&self) -> Result<(), AssetLibraryError> {
        let bytes =
            serde_json::to_vec_pretty(&self.index).map_err(AssetLibraryError::SerializeIndex)?;
        let tmp_path = self.root.join(INDEX_TMP_FILE);
        write_file_synced(&tmp_path, &bytes)?;
        fs::rename(&tmp_path, &self.index_path).map_err(|source| AssetLibraryError::Replace {
            path: self.index_path.clone(),
            source,
        })?;
        sync_dir(&self.root)?;
        Ok(())
    }

    fn warnings_for_new_asset(&self, byte_len: u64) -> Vec<AssetWarning> {
        let mut warnings = Vec::new();
        if byte_len > self.limits.per_asset_soft_cap_bytes {
            warnings.push(AssetWarning::PerAssetSoftCapExceeded {
                limit_bytes: self.limits.per_asset_soft_cap_bytes,
            });
        }
        let library_bytes = self
            .index
            .records()
            .iter()
            .map(|record| record.byte_len)
            .sum::<u64>()
            .saturating_add(byte_len);
        if library_bytes > self.limits.library_soft_cap_bytes {
            warnings.push(AssetWarning::LibrarySoftCapExceeded {
                limit_bytes: self.limits.library_soft_cap_bytes,
            });
        }
        warnings
    }
}

#[derive(Debug)]
struct DiscoveredObject {
    path_hash: String,
    actual_hash: String,
    byte_len: u64,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScannedMetadata {
    mime_type: String,
    intrinsic_width: Option<u32>,
    intrinsic_height: Option<u32>,
    duration_us: Option<u64>,
    frame_count: Option<u32>,
}

fn load_index(index_path: &Path) -> Result<Option<AssetIndex>, AssetLibraryError> {
    if !index_path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(index_path).map_err(|source| AssetLibraryError::Read {
        path: index_path.to_path_buf(),
        source,
    })?;
    match serde_json::from_str::<AssetIndex>(&raw) {
        Ok(mut index) => {
            index.version = INDEX_VERSION;
            Ok(Some(index))
        }
        Err(source) => {
            warn!(
                path = %index_path.display(),
                %source,
                "Asset index is invalid; rebuilding from objects"
            );
            Ok(None)
        }
    }
}

fn index_change_events(previous: &AssetIndex, next: &AssetIndex) -> Vec<AssetEvent> {
    let previous_by_id: HashMap<AssetId, MediaAssetRecord> = previous
        .records()
        .iter()
        .cloned()
        .map(|record| (record.id, record))
        .collect();
    let next_ids: HashSet<AssetId> = next.records().iter().map(|record| record.id).collect();
    let mut events = Vec::new();

    for record in next.records() {
        match previous_by_id.get(&record.id) {
            Some(previous) if previous != record => {
                events.push(AssetEvent::Modified {
                    record: record.clone(),
                });
            }
            None => {
                events.push(AssetEvent::Added {
                    record: record.clone(),
                });
            }
            Some(_) => {}
        }
    }

    for id in previous_by_id.keys().copied() {
        if !next_ids.contains(&id) {
            events.push(AssetEvent::Removed { asset_id: id });
        }
    }

    events
}

fn scan_metadata(
    bytes: &[u8],
    type_hint: Option<AssetTypeHint>,
    stream_url_policy: &StreamUrlPolicy,
) -> Result<ScannedMetadata, AssetLibraryError> {
    let Some(mime_type) = sniff_mime(bytes, type_hint, stream_url_policy) else {
        return Err(AssetLibraryError::UnsupportedMediaType {
            reason: "unsupported or unverifiable file signature".to_owned(),
        });
    };

    let mut metadata = ScannedMetadata {
        mime_type,
        intrinsic_width: None,
        intrinsic_height: None,
        duration_us: None,
        frame_count: None,
    };

    if is_thumbnail_source(&metadata.mime_type) {
        let image = image::load_from_memory(bytes).map_err(AssetLibraryError::DecodeImage)?;
        let (width, height) = image.dimensions();
        metadata.intrinsic_width = Some(width);
        metadata.intrinsic_height = Some(height);
    }

    Ok(metadata)
}

fn sniff_mime(
    bytes: &[u8],
    type_hint: Option<AssetTypeHint>,
    stream_url_policy: &StreamUrlPolicy,
) -> Option<String> {
    if type_hint == Some(AssetTypeHint::Lottie) && is_json(bytes) {
        return Some("application/json".to_owned());
    }
    if type_hint == Some(AssetTypeHint::Stream)
        && stream_url_from_bytes_with_policy(bytes, stream_url_policy).is_some()
    {
        return Some("application/vnd.hypercolor.stream-url".to_owned());
    }
    if is_png(bytes) {
        return Some(if is_apng(bytes) {
            "image/apng".to_owned()
        } else {
            "image/png".to_owned()
        });
    }
    if is_jpeg(bytes) {
        return Some("image/jpeg".to_owned());
    }
    if is_webp(bytes) {
        return Some("image/webp".to_owned());
    }
    if is_gif(bytes) {
        return Some("image/gif".to_owned());
    }
    if is_mp4(bytes) {
        return Some("video/mp4".to_owned());
    }
    if is_webm(bytes) {
        return Some("video/webm".to_owned());
    }
    if is_lottie(bytes) {
        return Some("application/json".to_owned());
    }
    None
}

fn is_thumbnail_source(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "image/png" | "image/apng" | "image/jpeg" | "image/webp" | "image/gif"
    )
}

pub fn stream_url_from_bytes(bytes: &[u8]) -> Option<String> {
    stream_url_from_bytes_with_policy(bytes, &StreamUrlPolicy::default())
}

#[must_use]
pub fn stream_url_from_bytes_with_policy(
    bytes: &[u8],
    stream_url_policy: &StreamUrlPolicy,
) -> Option<String> {
    let raw = std::str::from_utf8(bytes).ok()?;
    let url = raw.lines().map(str::trim).find(|line| !line.is_empty())?;
    stream_url_policy.allows_url(url).then(|| url.to_owned())
}

fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ipv6) => ipv6.to_ipv4_mapped().map_or(IpAddr::V6(ipv6), IpAddr::V4),
        IpAddr::V4(_) => ip,
    }
}

/// `Url::host_str` wraps IPv6 literals in brackets, which `IpAddr` parsing rejects.
fn host_as_ip(host: &str) -> Option<IpAddr> {
    host.strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host)
        .parse::<IpAddr>()
        .ok()
}

fn is_local_hostname(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost")
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_unspecified())
        }
        IpAddr::V6(ip) => {
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local())
        }
    }
}

fn cidr_prefix_valid(network: IpAddr, prefix: u8) -> bool {
    match network {
        IpAddr::V4(_) => prefix <= 32,
        IpAddr::V6(_) => prefix <= 128,
    }
}

fn cidr_contains(network: IpAddr, prefix: u8, client: IpAddr) -> bool {
    match (network, client) {
        (IpAddr::V4(network), IpAddr::V4(client)) => {
            masked_v4(network, prefix) == masked_v4(client, prefix)
        }
        (IpAddr::V6(network), IpAddr::V6(client)) => {
            masked_v6(network, prefix) == masked_v6(client, prefix)
        }
        _ => false,
    }
}

fn masked_v4(address: Ipv4Addr, prefix: u8) -> u32 {
    let bits = u32::from(address);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    bits & mask
}

fn masked_v6(address: Ipv6Addr, prefix: u8) -> u128 {
    let bits = u128::from(address);
    let mask = if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    };
    bits & mask
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}

fn is_apng(bytes: &[u8]) -> bool {
    if !is_png(bytes) {
        return false;
    }

    let mut offset = 8usize;
    while offset + 12 <= bytes.len() {
        let chunk_len = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]);
        let Ok(chunk_len) = usize::try_from(chunk_len) else {
            return false;
        };
        let chunk_type_start = offset + 4;
        let chunk_type_end = chunk_type_start + 4;
        let chunk_end = chunk_type_end.saturating_add(chunk_len).saturating_add(4);
        if chunk_end > bytes.len() {
            return false;
        }
        let chunk_type = &bytes[chunk_type_start..chunk_type_end];
        if chunk_type == b"acTL" {
            return true;
        }
        if chunk_type == b"IDAT" {
            return false;
        }
        offset = chunk_end;
    }
    false
}

fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 3 && bytes[0..3] == [0xff, 0xd8, 0xff]
}

fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP"
}

fn is_gif(bytes: &[u8]) -> bool {
    bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")
}

fn is_mp4(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[4..8] == b"ftyp"
}

fn is_webm(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3])
}

fn is_json(bytes: &[u8]) -> bool {
    serde_json::from_slice::<Value>(bytes).is_ok()
}

fn is_lottie(bytes: &[u8]) -> bool {
    let Ok(Value::Object(object)) = serde_json::from_slice::<Value>(bytes) else {
        return false;
    };
    object.contains_key("v") && object.contains_key("layers")
}

fn sanitize_name(name: &str) -> String {
    let file_name = Path::new(name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(name)
        .trim();
    let mut sanitized = String::with_capacity(file_name.len().min(128));
    let mut last_was_separator = false;

    for character in file_name.chars() {
        let replacement =
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | ' ') {
                character
            } else {
                '_'
            };
        let is_separator = replacement == '_' || replacement == ' ';
        if is_separator && last_was_separator {
            continue;
        }
        sanitized.push(replacement);
        last_was_separator = is_separator;
        if sanitized.len() >= 128 {
            break;
        }
    }

    let sanitized = sanitized.trim_matches([' ', '.', '_']).to_owned();
    if sanitized.is_empty() {
        "asset".to_owned()
    } else {
        sanitized
    }
}

fn sanitize_tags(tags: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    tags.into_iter()
        .map(|tag| tag.trim().to_owned())
        .filter(|tag| !tag.is_empty())
        .map(|mut tag| {
            tag.truncate(64);
            tag
        })
        .filter(|tag| seen.insert(tag.clone()))
        .collect()
}

fn warn_if_over_soft_cap(
    record: &MediaAssetRecord,
    object_path: &Path,
    limits: AssetLibraryLimits,
) {
    if record.byte_len > limits.per_asset_soft_cap_bytes {
        warn!(
            asset_id = %record.id,
            path = %object_path.display(),
            byte_len = record.byte_len,
            limit = limits.per_asset_soft_cap_bytes,
            "Asset exceeds per-asset soft cap"
        );
    }
}

fn create_dir(path: &Path) -> Result<(), AssetLibraryError> {
    fs::create_dir_all(path).map_err(|source| AssetLibraryError::CreateDir {
        path: path.to_path_buf(),
        source,
    })
}

fn write_file_synced(path: &Path, bytes: &[u8]) -> Result<(), AssetLibraryError> {
    let mut file = File::create(path).map_err(|source| AssetLibraryError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    file.write_all(bytes)
        .map_err(|source| AssetLibraryError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    file.sync_all().map_err(|source| AssetLibraryError::Sync {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

#[cfg(unix)]
fn sync_dir(path: &Path) -> Result<(), AssetLibraryError> {
    let file = File::open(path).map_err(|source| AssetLibraryError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    file.sync_all().map_err(|source| AssetLibraryError::Sync {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn sync_dir(_path: &Path) -> Result<(), AssetLibraryError> {
    Ok(())
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    if let Ok(metadata) = fs::metadata(path) {
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o700);
        let _ = fs::set_permissions(path, permissions);
    }
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &Path) {}

fn now_utc() -> DateTime<Utc> {
    DateTime::<Utc>::from(SystemTime::now())
}

fn bytes_len(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).expect("asset byte length fits in u64")
}
