//! Effect covers named by the SHA-256 of their bytes.
//!
//! A cover is the curated override when one exists, else the `<meta cover>`
//! image an HTML effect ships inline, else a frame the daemon captured while
//! the effect owned the canvas (see [`crate::effect_cover_keeper`]). Its
//! digest travels in the cover URL (`?v=`) and the `ETag`, so every cache
//! between the daemon and a browser keeps one copy per image and can check
//! the bytes it hands back.
//!
//! Digests are memoized per effect against the size and modification time of
//! the files a cover comes from, so listing effects reads cover bytes only
//! when one of those files changed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::SystemTime;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use sha2::{Digest, Sha256};
use tracing::warn;

use hypercolor_types::effect::{EffectMetadata, EffectSource};
use serde::{Deserialize, Serialize};

use crate::resource_summary::{effect_cover_image_path, html_effect_source_path};

/// Content type of curated override art.
const CURATED_CONTENT_TYPE: &str = "image/webp";

/// A cover's bytes and the identity they hash to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EffectCover {
    pub(crate) identity: CoverIdentity,
    pub(crate) bytes: Vec<u8>,
}

/// What names a cover: the lowercase hex SHA-256 of its bytes, and its type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CoverIdentity {
    pub(crate) sha256: String,
    pub(crate) content_type: String,
}

impl CoverIdentity {
    /// The strong `ETag` for this cover, quoted.
    pub(crate) fn etag(&self) -> String {
        format!("\"sha256-{}\"", self.sha256)
    }
}

/// The size and modification time of one file a cover is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileStamp {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
}

impl FileStamp {
    fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        metadata.is_file().then(|| Self {
            path: path.to_path_buf(),
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

/// Everything whose change can change an effect's cover: the files it is
/// read from, and the version a built-in effect's capture belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CoverSources {
    curated: Option<FileStamp>,
    html: Option<FileStamp>,
    captured_image: Option<FileStamp>,
    captured_record: Option<FileStamp>,
    version: String,
}

impl CoverSources {
    fn of(metadata: &EffectMetadata) -> Self {
        let captured = captured_paths(metadata);
        Self {
            curated: effect_cover_image_path(metadata).and_then(|path| FileStamp::of(&path)),
            html: html_effect_source_path(metadata).and_then(|path| FileStamp::of(path)),
            captured_image: captured
                .as_ref()
                .and_then(|(image, _)| FileStamp::of(image)),
            captured_record: captured
                .as_ref()
                .and_then(|(_, record)| FileStamp::of(record)),
            version: metadata.version.clone(),
        }
    }

    const fn is_empty(&self) -> bool {
        self.curated.is_none() && self.html.is_none() && self.captured_image.is_none()
    }
}

/// Content type of covers the daemon captures from the canvas.
pub(crate) const CAPTURED_CONTENT_TYPE: &str = "image/jpeg";

/// What a captured cover was taken from, so a changed effect is captured
/// again instead of keeping a picture of its old self.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CapturedRecord {
    source: String,
}

/// Where captured covers live: beside the curated overrides, one image and
/// one record per effect id.
fn captured_root() -> Option<PathBuf> {
    Some(
        hypercolor_core::effect::bundled_screenshots_root()
            .parent()?
            .join("captured"),
    )
}

fn captured_paths(metadata: &EffectMetadata) -> Option<(PathBuf, PathBuf)> {
    Some(captured_paths_in(&captured_root()?, metadata))
}

fn captured_paths_in(root: &Path, metadata: &EffectMetadata) -> (PathBuf, PathBuf) {
    let id = metadata.id.to_string();
    (
        root.join(format!("{id}.jpg")),
        root.join(format!("{id}.json")),
    )
}

/// Names the effect source a captured cover belongs to: the HTML file's
/// size and modification time, or the version of a built-in effect.
fn capture_source_key(metadata: &EffectMetadata) -> String {
    match &metadata.source {
        EffectSource::Html { path } => FileStamp::of(path).map_or_else(
            || format!("html:{}", path.display()),
            |stamp| {
                let modified = stamp
                    .modified
                    .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map_or(0, |since| since.as_millis());
                format!("html:{}:{modified}", stamp.len)
            },
        ),
        EffectSource::Native { .. } | EffectSource::Shader { .. } => {
            format!("version:{}", metadata.version)
        }
    }
}

/// Whether the effect has no cover of its own and no current captured one.
///
/// Blocking: reads the capture record.
pub(crate) fn wants_capture(metadata: &EffectMetadata) -> bool {
    if effect_cover_image_path(metadata).is_some() {
        return false;
    }
    if read_inline(metadata).is_some() {
        return false;
    }
    read_captured(metadata).is_none()
}

/// Keep a frame captured while the effect owned the canvas as its cover.
///
/// Blocking: writes two files, each through a temporary file and a rename,
/// so a crash never leaves a cover without its record or a torn image.
pub(crate) fn store_captured(metadata: &EffectMetadata, jpeg: &[u8]) -> std::io::Result<()> {
    let stored = match captured_root() {
        Some(root) => store_captured_in(&root, metadata, jpeg),
        None => Ok(()),
    };
    // The next lookup reads the new cover rather than a memo taken mid-write.
    forget(metadata);
    stored
}

fn forget(metadata: &EffectMetadata) {
    IDENTITIES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&metadata.id.to_string());
}

fn store_captured_in(root: &Path, metadata: &EffectMetadata, jpeg: &[u8]) -> std::io::Result<()> {
    let (image, record) = captured_paths_in(root, metadata);
    if let Some(dir) = image.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = serde_json::to_vec(&CapturedRecord {
        source: capture_source_key(metadata),
    })
    .map_err(std::io::Error::other)?;
    write_replacing(&image, jpeg)?;
    write_replacing(&record, &body)
}

fn write_replacing(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let staging = path.with_extension("partial");
    std::fs::write(&staging, bytes)?;
    std::fs::rename(&staging, path)
}

fn read_captured(metadata: &EffectMetadata) -> Option<EffectCover> {
    read_captured_in(&captured_root()?, metadata)
}

fn read_captured_in(root: &Path, metadata: &EffectMetadata) -> Option<EffectCover> {
    let (image, record) = captured_paths_in(root, metadata);
    let record: CapturedRecord = serde_json::from_slice(&std::fs::read(record).ok()?).ok()?;
    if record.source != capture_source_key(metadata) {
        return None;
    }
    let bytes = std::fs::read(image).ok()?;
    Some(named(CAPTURED_CONTENT_TYPE.to_owned(), bytes))
}

type IdentityMemo = HashMap<String, (CoverSources, Option<CoverIdentity>)>;

static IDENTITIES: LazyLock<Mutex<IdentityMemo>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The identity of an effect's cover, or `None` when it has none.
///
/// Reads cover bytes only on the first call per effect and again after one
/// of its source files changes.
pub(crate) fn cover_identity(metadata: &EffectMetadata) -> Option<CoverIdentity> {
    let sources = CoverSources::of(metadata);
    if sources.is_empty() {
        return None;
    }
    let key = metadata.id.to_string();
    if let Some((memo_sources, identity)) = IDENTITIES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        && *memo_sources == sources
    {
        return identity.clone();
    }
    let identity = read_cover(metadata).map(|cover| cover.identity);
    remember(key, sources, identity.clone());
    identity
}

/// Read an effect's cover bytes, preferring curated art over the image the
/// effect ships inline so a local override can replace bundled art, and
/// either over a frame the daemon captured.
///
/// Blocking: callers on an async runtime run it on the blocking pool.
pub(crate) fn read_cover(metadata: &EffectMetadata) -> Option<EffectCover> {
    let sources = CoverSources::of(metadata);
    let cover = read_curated(metadata)
        .or_else(|| read_inline(metadata))
        .or_else(|| read_captured(metadata));
    remember(
        metadata.id.to_string(),
        sources,
        cover.as_ref().map(|cover| cover.identity.clone()),
    );
    cover
}

fn remember(key: String, sources: CoverSources, identity: Option<CoverIdentity>) {
    IDENTITIES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, (sources, identity));
}

fn read_curated(metadata: &EffectMetadata) -> Option<EffectCover> {
    let path = effect_cover_image_path(metadata)?;
    match std::fs::read(&path) {
        Ok(bytes) => Some(named(CURATED_CONTENT_TYPE.to_owned(), bytes)),
        Err(error) => {
            warn!(
                path = %path.display(),
                error = %error,
                "Failed to read curated effect cover; falling back to inline cover"
            );
            None
        }
    }
}

/// Decode the `data:image/<type>;base64,` cover an HTML effect declares in
/// its `<meta cover>` tag.
fn read_inline(metadata: &EffectMetadata) -> Option<EffectCover> {
    let path = html_effect_source_path(metadata)?;
    let html = std::fs::read_to_string(path).ok()?;
    let cover = hypercolor_core::effect::parse_html_effect_metadata(&html).cover?;
    let (declaration, payload) = cover.split_once(";base64,")?;
    let content_type = declaration.strip_prefix("data:")?.to_ascii_lowercase();
    if !content_type.starts_with("image/") {
        return None;
    }
    match BASE64_STANDARD.decode(payload) {
        Ok(bytes) => Some(named(content_type, bytes)),
        Err(error) => {
            warn!(
                effect = %metadata.name,
                error = %error,
                "Effect declared an inline cover that is not valid base64"
            );
            None
        }
    }
}

fn named(content_type: String, bytes: Vec<u8>) -> EffectCover {
    EffectCover {
        identity: CoverIdentity {
            sha256: sha256_hex(&bytes),
            content_type,
        },
        bytes,
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut output, "{byte:02x}").expect("writing into a String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_are_lowercase_hex_sha256() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    fn native(version: &str) -> EffectMetadata {
        EffectMetadata {
            id: hypercolor_types::effect::EffectId::new(uuid::Uuid::from_u128(7)),
            name: "Plasma".to_owned(),
            author: "Hypercolor".to_owned(),
            version: version.to_owned(),
            description: String::new(),
            category: hypercolor_types::effect::EffectCategory::default(),
            tags: Vec::new(),
            controls: Vec::new(),
            presets: Vec::new(),
            audio_reactive: false,
            screen_reactive: false,
            input_reactive: false,
            source: EffectSource::Native {
                path: PathBuf::from("plasma"),
            },
            license: None,
        }
    }

    #[test]
    fn a_captured_cover_is_named_by_its_bytes() {
        let root = tempfile::tempdir().expect("temp dir");
        let effect = native("1.0.0");
        store_captured_in(root.path(), &effect, b"jpeg-bytes").expect("store");
        let cover = read_captured_in(root.path(), &effect).expect("captured cover");
        assert_eq!(cover.bytes, b"jpeg-bytes");
        assert_eq!(cover.identity.sha256, sha256_hex(b"jpeg-bytes"));
        assert_eq!(cover.identity.content_type, CAPTURED_CONTENT_TYPE);
    }

    #[test]
    fn a_changed_effect_drops_its_old_capture() {
        let root = tempfile::tempdir().expect("temp dir");
        store_captured_in(root.path(), &native("1.0.0"), b"old frame").expect("store");
        assert!(read_captured_in(root.path(), &native("1.1.0")).is_none());
    }

    #[test]
    fn a_capture_leaves_no_partial_files() {
        let root = tempfile::tempdir().expect("temp dir");
        store_captured_in(root.path(), &native("1.0.0"), b"frame").expect("store");
        let leftovers = std::fs::read_dir(root.path())
            .expect("list")
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "partial"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn etags_name_the_digest() {
        let identity = CoverIdentity {
            sha256: "ab".repeat(32),
            content_type: "image/webp".to_owned(),
        };
        assert_eq!(identity.etag(), format!("\"sha256-{}\"", "ab".repeat(32)));
    }
}
