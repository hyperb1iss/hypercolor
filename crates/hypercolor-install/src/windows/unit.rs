//! Producing a Windows release unit (RFC 57, B1.3, B1.4.1, B2.2 and B2.5).
//!
//! A unit is the tree the installer lays out under
//! `%ProgramFiles%\Hypercolor\units\<digest>\`: the app, daemon and CLI at
//! its root beside the DLLs they load, the UI and bundled effects, the
//! hardware tools, the store inventory every release ships at
//! `share/hypercolor/durable-stores.json`, and the license notices. The
//! producer copies each member from the build's outputs, hashes the bytes
//! it writes, classes every PE image a program and everything else data,
//! names the programs that keep their vendor's signature, and writes
//! `manifest.json` last. The manifest is checked with the same parser the
//! store stages units with before the producer reports success, so a unit
//! it produces is one the store accepts.
//!
//! Code signing changes a program's bytes, so every program must already be
//! signed when the unit is produced.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

use hypercolor_platform_fs::windows_file_name_is_valid;
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

use super::error::ReleasePayloadError;
use super::manifest::{
    MANIFEST_NAME, PermissionClass, ValidatedManifest, WindowsVendor, fold_case,
};
use crate::model::UnitId;

/// Where every release keeps its store inventory, relative to the unit.
pub const WINDOWS_DURABLE_STORES_PATH: &str = "share/hypercolor/durable-stores.json";
/// The engine's own store inventory, relative to the source tree.
const ENGINE_DURABLE_STORES: &str = "packaging/managed/durable-stores.json";
/// Programs at the unit root, which every manifest lists.
const ROOT_PROGRAMS: [&str; 3] = [
    "hypercolor-app.exe",
    "hypercolor-daemon.exe",
    "hypercolor.exe",
];
const COPY_BUFFER_BYTES: usize = 1024 * 1024;
/// A DOS header is 64 bytes, and its last field locates the PE header.
const DOS_HEADER_BYTES: usize = 0x40;
const PE_OFFSET_FIELD: usize = 0x3C;

/// Where one placed member comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Source {
    /// A file in the directory the release executables were built into.
    Built(&'static str),
    /// A path under the staged app bundle assets
    /// (`stage-app-bundle-assets.ps1`).
    Staged(&'static str),
    /// A path in the engine source tree.
    Repository(&'static str),
}

/// One file or directory tree the unit carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Placement {
    /// Unit-relative destination.
    pub(super) path: &'static str,
    pub(super) source: Source,
    /// The whole directory tree at the source, rather than one file.
    pub(super) tree: bool,
}

const fn file(path: &'static str, source: Source) -> Placement {
    Placement {
        path,
        source,
        tree: false,
    }
}

const fn tree(path: &'static str, source: Source) -> Placement {
    Placement {
        path,
        source,
        tree: true,
    }
}

/// The unit tree: the flat layout the installer has always laid out, as
/// `crates/hypercolor-app/tauri*.conf.json` describe it, plus the notices
/// and the store inventory a unit carries like every other release.
pub(super) const LAYOUT: [Placement; 26] = [
    file("hypercolor-app.exe", Source::Built("hypercolor-app.exe")),
    file(
        "hypercolor-daemon.exe",
        Source::Built("hypercolor-daemon.exe"),
    ),
    file("hypercolor.exe", Source::Built("hypercolor.exe")),
    file("libEGL.dll", Source::Staged("dlls/libEGL.dll")),
    file("libGLESv2.dll", Source::Staged("dlls/libGLESv2.dll")),
    file("msvcp140.dll", Source::Staged("dlls/msvcp140.dll")),
    file("vcruntime140.dll", Source::Staged("dlls/vcruntime140.dll")),
    file(
        "vcruntime140_1.dll",
        Source::Staged("dlls/vcruntime140_1.dll"),
    ),
    tree("ui", Source::Staged("ui")),
    tree("effects/bundled", Source::Staged("effects")),
    file(
        "tools/hypercolor-smbus-service.exe",
        Source::Built("hypercolor-smbus-service.exe"),
    ),
    file(
        "tools/hypercolor-update-bootstrap.exe",
        Source::Built("hypercolor-update-bootstrap.exe"),
    ),
    file(
        "tools/hypercolor-windows-helper.exe",
        Source::Built("hypercolor-windows-helper.exe"),
    ),
    file("tools/msvcp140.dll", Source::Staged("tools/msvcp140.dll")),
    file(
        "tools/vcruntime140.dll",
        Source::Staged("tools/vcruntime140.dll"),
    ),
    file(
        "tools/vcruntime140_1.dll",
        Source::Staged("tools/vcruntime140_1.dll"),
    ),
    tree("tools/pawnio", Source::Staged("tools/pawnio")),
    file(
        "tools/install-windows-service.ps1",
        Source::Repository("scripts/install-windows-service.ps1"),
    ),
    file(
        "tools/uninstall-windows-service.ps1",
        Source::Repository("scripts/uninstall-windows-service.ps1"),
    ),
    file(
        "tools/diagnose-windows.ps1",
        Source::Repository("scripts/diagnose-windows.ps1"),
    ),
    file(
        "tools/install-windows-smbus-service.ps1",
        Source::Repository("scripts/install-windows-smbus-service.ps1"),
    ),
    file(
        "tools/install-pawnio-modules.ps1",
        Source::Repository("scripts/install-pawnio-modules.ps1"),
    ),
    file(
        "tools/install-bundled-pawnio.ps1",
        Source::Repository("scripts/install-bundled-pawnio.ps1"),
    ),
    file(
        "tools/install-windows-hardware-support.ps1",
        Source::Repository("scripts/install-windows-hardware-support.ps1"),
    ),
    file("LICENSE", Source::Repository("LICENSE")),
    file("NOTICE", Source::Repository("NOTICE")),
];

/// Programs we redistribute with their vendor's signature, by exact unit
/// path. Every other PE image in a unit carries ours.
pub(super) const VENDOR_SIGNED: [(&str, WindowsVendor); 7] = [
    ("msvcp140.dll", WindowsVendor::Microsoft),
    ("vcruntime140.dll", WindowsVendor::Microsoft),
    ("vcruntime140_1.dll", WindowsVendor::Microsoft),
    ("tools/msvcp140.dll", WindowsVendor::Microsoft),
    ("tools/vcruntime140.dll", WindowsVendor::Microsoft),
    ("tools/vcruntime140_1.dll", WindowsVendor::Microsoft),
    ("tools/pawnio/PawnIO_setup.exe", WindowsVendor::Pawnio),
];

/// What one Windows release unit is built from.
#[derive(Debug, Clone)]
pub struct WindowsUnitSources {
    /// The release version every program reports.
    pub version: String,
    /// The Rust target the programs were built for, such as
    /// `x86_64-pc-windows-msvc`.
    pub rust_target: String,
    /// The directory holding the five release executables: the app,
    /// daemon, CLI, SMBus broker and helper, already signed.
    pub bin_dir: PathBuf,
    /// The staged app bundle assets `stage-app-bundle-assets.ps1` writes
    /// (`target/bundle-stage`): `ui/`, `effects/`, `dlls/` and `tools/`.
    pub stage_dir: PathBuf,
    /// The engine source tree, for the tool scripts, `LICENSE`, `NOTICE`
    /// and the engine's store inventory.
    pub source_root: PathBuf,
    /// Store inventories a downstream build adds, each `{"stores": [...]}`
    /// naming stores the engine does not.
    pub durable_store_overlays: Vec<PathBuf>,
    /// Notices of third-party code a downstream build links, shipped as
    /// `third-party/`.
    pub third_party_notices: Option<PathBuf>,
}

/// A produced unit: its tree and the digest that names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsUnit {
    unit: UnitId,
    root: PathBuf,
    vendor_signed: Vec<(String, WindowsVendor)>,
}

impl WindowsUnit {
    /// The SHA-256 of the unit's `manifest.json`, which names the unit.
    #[must_use]
    pub fn unit_id(&self) -> &UnitId {
        &self.unit
    }

    /// The unit tree, `manifest.json` at its root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The programs that keep their vendor's signature, by unit path.
    #[must_use]
    pub fn vendor_signed(&self) -> &[(String, WindowsVendor)] {
        &self.vendor_signed
    }
}

/// Why a unit could not be produced.
#[derive(Debug, thiserror::Error)]
pub enum WindowsUnitError {
    /// An input is missing, malformed or holds something a unit cannot.
    #[error("{0}")]
    InvalidSources(String),
    /// Reading an input or writing the unit failed.
    #[error("failed to {operation} {}: {source}", path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The manifest the producer wrote is one the store would refuse.
    #[error("the produced manifest is refused: {0}")]
    Manifest(#[from] ReleasePayloadError),
}

/// Produce one Windows release unit at `output`, which must not exist yet.
///
/// On failure the producer removes the directory it created at `output`.
///
/// # Errors
///
/// Returns an error for a missing or malformed input, an input holding a
/// link or a name Windows reads differently, two members a case-insensitive
/// volume merges, a write failure, or a manifest the store would refuse.
pub fn produce_windows_unit(
    sources: &WindowsUnitSources,
    output: &Path,
) -> Result<WindowsUnit, WindowsUnitError> {
    let platform = platform_of(&sources.rust_target)?;
    let plan = Plan::from_sources(sources)?;
    match fs::symlink_metadata(output) {
        Ok(_) => {
            return Err(invalid(format!(
                "{} already exists; a unit is produced into a new directory",
                output.display()
            )));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(io_error("inspect", output, source)),
    }
    fs::create_dir(output).map_err(|source| io_error("create", output, source))?;
    match write_unit(&plan, sources, &platform, output) {
        Ok(unit) => Ok(unit),
        Err(error) => {
            // The directory is new and holds only what this call wrote.
            let _ = fs::remove_dir_all(output);
            Err(error)
        }
    }
}

/// `windows-<arch>` for an MSVC Windows target.
fn platform_of(rust_target: &str) -> Result<String, WindowsUnitError> {
    match rust_target.strip_suffix("-pc-windows-msvc") {
        Some(arch)
            if !arch.is_empty() && arch.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') =>
        {
            Ok(format!("windows-{arch}"))
        }
        _ => Err(invalid(format!(
            "{rust_target:?} is not an MSVC Windows target"
        ))),
    }
}

/// Every member of the unit and where it comes from, checked before
/// anything is written.
#[derive(Debug, Default)]
struct Plan {
    directories: BTreeSet<String>,
    files: BTreeMap<String, PathBuf>,
    generated: BTreeMap<String, Vec<u8>>,
    /// Every planned path, folded as NTFS compares names, with its original
    /// spelling.
    folded: BTreeMap<String, String>,
}

impl Plan {
    fn from_sources(sources: &WindowsUnitSources) -> Result<Self, WindowsUnitError> {
        let mut plan = Self::default();
        for placement in LAYOUT {
            let source = match placement.source {
                Source::Built(name) => sources.bin_dir.join(name),
                Source::Staged(path) => sources.stage_dir.join(path),
                Source::Repository(path) => sources.source_root.join(path),
            };
            if placement.tree {
                plan.add_tree(placement.path, &source)?;
            } else {
                plan.add_file(placement.path, &source)?;
            }
        }
        if let Some(notices) = &sources.third_party_notices {
            let before = plan.files.len();
            plan.add_tree("third-party", notices)?;
            if plan.files.len() == before {
                return Err(invalid(format!(
                    "the third-party notices at {} hold no files",
                    notices.display()
                )));
            }
        }
        let inventory = durable_store_inventory(
            &sources.source_root.join(ENGINE_DURABLE_STORES),
            &sources.durable_store_overlays,
        )?;
        plan.add_generated(WINDOWS_DURABLE_STORES_PATH, inventory)?;
        for (path, _) in VENDOR_SIGNED {
            if !plan.files.contains_key(path) {
                return Err(invalid(format!(
                    "the unit has no {path}, which it ships with its vendor's signature"
                )));
            }
        }
        Ok(plan)
    }

    fn claim(&mut self, path: &str, directory: bool) -> Result<bool, WindowsUnitError> {
        if path
            .split('/')
            .any(|name| !windows_file_name_is_valid(name))
            || fold_case(path) == fold_case(MANIFEST_NAME)
        {
            return Err(invalid(format!(
                "{path:?} is not a unit path Windows reads as written"
            )));
        }
        match self.folded.get(&fold_case(path)) {
            Some(existing) if directory && existing == path && self.directories.contains(path) => {
                Ok(false)
            }
            Some(existing) if existing == path => {
                Err(invalid(format!("{path} is placed in the unit twice")))
            }
            Some(existing) => Err(invalid(format!(
                "{path} and {existing} name the same member on a case-insensitive volume"
            ))),
            None => {
                self.folded.insert(fold_case(path), path.to_owned());
                Ok(true)
            }
        }
    }

    fn add_ancestors(&mut self, path: &str) -> Result<(), WindowsUnitError> {
        let mut parent = String::new();
        let components: Vec<&str> = path.split('/').collect();
        for name in &components[..components.len() - 1] {
            if !parent.is_empty() {
                parent.push('/');
            }
            parent.push_str(name);
            let ancestor = parent.clone();
            self.add_directory(&ancestor)?;
        }
        Ok(())
    }

    fn add_directory(&mut self, path: &str) -> Result<(), WindowsUnitError> {
        if self.claim(path, true)? {
            self.directories.insert(path.to_owned());
        }
        Ok(())
    }

    fn add_file(&mut self, path: &str, source: &Path) -> Result<(), WindowsUnitError> {
        let metadata =
            fs::symlink_metadata(source).map_err(|error| io_error("inspect", source, error))?;
        if !metadata.is_file() {
            return Err(invalid(format!(
                "{} must be a regular file for {path}",
                source.display()
            )));
        }
        self.add_ancestors(path)?;
        self.claim(path, false)?;
        self.files.insert(path.to_owned(), source.to_path_buf());
        Ok(())
    }

    fn add_generated(&mut self, path: &str, bytes: Vec<u8>) -> Result<(), WindowsUnitError> {
        self.add_ancestors(path)?;
        self.claim(path, false)?;
        self.generated.insert(path.to_owned(), bytes);
        Ok(())
    }

    fn add_tree(&mut self, path: &str, source: &Path) -> Result<(), WindowsUnitError> {
        let metadata =
            fs::symlink_metadata(source).map_err(|error| io_error("inspect", source, error))?;
        if !metadata.is_dir() {
            return Err(invalid(format!(
                "{} must be a directory for {path}",
                source.display()
            )));
        }
        self.add_ancestors(path)?;
        self.add_directory(path)?;
        let mut entries = fs::read_dir(source)
            .map_err(|error| io_error("list", source, error))?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| io_error("list", source, error))?;
        entries.sort();
        for name in entries {
            let Some(text) = name.to_str() else {
                return Err(invalid(format!(
                    "{} holds a name that is not Unicode",
                    source.display()
                )));
            };
            let child = source.join(&name);
            let member = format!("{path}/{text}");
            let metadata =
                fs::symlink_metadata(&child).map_err(|error| io_error("inspect", &child, error))?;
            if metadata.is_dir() {
                self.add_tree(&member, &child)?;
            } else if metadata.is_file() {
                self.add_file(&member, &child)?;
            } else {
                return Err(invalid(format!(
                    "{} is neither a regular file nor a directory",
                    child.display()
                )));
            }
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct ManifestOut<'a> {
    name: &'static str,
    version: &'a str,
    platform: &'a str,
    rust_target: &'a str,
    programs: [&'static str; 3],
    members: Vec<MemberOut>,
    vendor_signed: Vec<VendorSignedOut>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum MemberOut {
    Directory {
        path: String,
    },
    File {
        path: String,
        class: PermissionClass,
        size: u64,
        sha256: String,
    },
}

#[derive(Serialize)]
struct VendorSignedOut {
    path: String,
    vendor: WindowsVendor,
}

fn write_unit(
    plan: &Plan,
    sources: &WindowsUnitSources,
    platform: &str,
    output: &Path,
) -> Result<WindowsUnit, WindowsUnitError> {
    let mut directories: Vec<&String> = plan.directories.iter().collect();
    directories.sort_by_key(|path| components(path));
    for directory in &directories {
        let path = output.join(directory);
        fs::create_dir(&path).map_err(|source| io_error("create", &path, source))?;
    }

    let mut members: Vec<(Vec<String>, MemberOut)> = directories
        .iter()
        .map(|path| {
            (
                components(path),
                MemberOut::Directory {
                    path: (*path).clone(),
                },
            )
        })
        .collect();
    for (path, source) in &plan.files {
        let destination = output.join(path);
        let (size, sha256) = copy_hashed(source, &destination)?;
        let class = class_of(&destination, size)?;
        members.push((components(path), file_member(path, class, size, sha256)));
    }
    for (path, bytes) in &plan.generated {
        let destination = output.join(path);
        write_new(&destination, bytes)?;
        let class = class_of(&destination, bytes.len() as u64)?;
        let sha256 = hex::encode(Sha256::digest(bytes));
        members.push((
            components(path),
            file_member(path, class, bytes.len() as u64, sha256),
        ));
    }
    members.sort_by(|left, right| left.0.cmp(&right.0));

    let vendor_signed: Vec<(String, WindowsVendor)> = VENDOR_SIGNED
        .iter()
        .map(|(path, vendor)| ((*path).to_owned(), *vendor))
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .collect();
    let manifest = ManifestOut {
        name: "hypercolor",
        version: &sources.version,
        platform,
        rust_target: &sources.rust_target,
        programs: ROOT_PROGRAMS,
        members: members.into_iter().map(|(_, member)| member).collect(),
        vendor_signed: vendor_signed
            .iter()
            .map(|(path, vendor)| VendorSignedOut {
                path: path.clone(),
                vendor: *vendor,
            })
            .collect(),
    };
    let bytes = canonical_json(
        serde_json::to_value(&manifest)
            .map_err(|error| invalid(format!("cannot encode the manifest: {error}")))?,
    )?;

    // The store's own parser decides whether this is a unit, not the
    // producer's intent.
    let parsed = ValidatedManifest::parse(bytes.clone())?;
    if parsed.vendor_signed.into_iter().collect::<Vec<_>>() != vendor_signed {
        return Err(invalid(
            "the parsed manifest does not keep the producer's vendor list".to_owned(),
        ));
    }
    write_new(&output.join(MANIFEST_NAME), &bytes)?;
    Ok(WindowsUnit {
        unit: parsed.unit_id,
        root: output.to_path_buf(),
        vendor_signed,
    })
}

fn file_member(path: &str, class: PermissionClass, size: u64, sha256: String) -> MemberOut {
    MemberOut::File {
        path: path.to_owned(),
        class,
        size,
        sha256,
    }
}

fn components(path: &str) -> Vec<String> {
    path.split('/').map(str::to_owned).collect()
}

/// Copy `source` to a new `destination`, returning the size and SHA-256 of
/// the bytes written. Only the file's data is copied: no attributes,
/// alternate streams or security.
fn copy_hashed(source: &Path, destination: &Path) -> Result<(u64, String), WindowsUnitError> {
    let mut input = File::open(source).map_err(|error| io_error("open", source, error))?;
    let mut output =
        File::create_new(destination).map_err(|error| io_error("create", destination, error))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut size = 0_u64;
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|error| io_error("read", source, error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        output
            .write_all(&buffer[..read])
            .map_err(|error| io_error("write", destination, error))?;
        size += read as u64;
    }
    output
        .flush()
        .map_err(|error| io_error("write", destination, error))?;
    Ok((size, hex::encode(hasher.finalize())))
}

fn write_new(destination: &Path, bytes: &[u8]) -> Result<(), WindowsUnitError> {
    File::create_new(destination)
        .and_then(|mut file| file.write_all(bytes))
        .map_err(|error| io_error("write", destination, error))
}

/// A member that is a PE image is a program, whatever its name; anything
/// else is data.
fn class_of(path: &Path, size: u64) -> Result<PermissionClass, WindowsUnitError> {
    let mut file = File::open(path).map_err(|error| io_error("open", path, error))?;
    let image = is_pe_image(&mut file, size).map_err(|error| io_error("read", path, error))?;
    Ok(if image {
        PermissionClass::Program
    } else {
        PermissionClass::Data
    })
}

/// Whether the file holds a DOS header whose `e_lfanew` points at a PE
/// signature, which is what makes Windows load it as an image.
pub(super) fn is_pe_image(file: &mut File, size: u64) -> io::Result<bool> {
    if size < DOS_HEADER_BYTES as u64 {
        return Ok(false);
    }
    let mut dos = [0_u8; DOS_HEADER_BYTES];
    file.read_exact(&mut dos)?;
    if &dos[..2] != b"MZ" {
        return Ok(false);
    }
    let mut field = [0_u8; 4];
    field.copy_from_slice(&dos[PE_OFFSET_FIELD..DOS_HEADER_BYTES]);
    let offset = u64::from(u32::from_le_bytes(field));
    if offset.checked_add(4).is_none_or(|end| end > size) {
        return Ok(false);
    }
    let mut signature = [0_u8; 4];
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(&mut signature)?;
    Ok(&signature == b"PE\0\0")
}

/// The engine's store inventory with each overlay's stores appended, every
/// store name declared once, as the Linux and macOS units ship it.
fn durable_store_inventory(
    engine: &Path,
    overlays: &[PathBuf],
) -> Result<Vec<u8>, WindowsUnitError> {
    let mut inventory = read_json(engine)?;
    let mut declared = BTreeSet::new();
    for name in store_names(&inventory, engine)? {
        if !declared.insert(name.clone()) {
            return Err(invalid(format!(
                "{} declares store {name:?} twice",
                engine.display()
            )));
        }
    }
    for overlay in overlays {
        let extra = read_json(overlay)?;
        if !extra.as_object().is_some_and(|object| {
            object.len() == 1 && object.get("stores").is_some_and(Value::is_array)
        }) {
            return Err(invalid(format!(
                "{} must hold exactly {{\"stores\": [...]}}",
                overlay.display()
            )));
        }
        for name in store_names(&extra, overlay)? {
            if !declared.insert(name.clone()) {
                return Err(invalid(format!(
                    "{} declares store {name:?} again",
                    overlay.display()
                )));
            }
        }
        let stores = extra["stores"].as_array().cloned().unwrap_or_default();
        if let Some(target) = inventory["stores"].as_array_mut() {
            target.extend(stores);
        }
    }
    canonical_json(inventory)
}

fn store_names(inventory: &Value, path: &Path) -> Result<Vec<String>, WindowsUnitError> {
    let stores = inventory
        .get("stores")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(format!("{} holds no stores list", path.display())))?;
    stores
        .iter()
        .map(|store| {
            store
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    invalid(format!(
                        "{} must declare each store as an object with a name",
                        path.display()
                    ))
                })
        })
        .collect()
}

fn read_json(path: &Path) -> Result<Value, WindowsUnitError> {
    let bytes = fs::read(path).map_err(|error| io_error("read", path, error))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| invalid(format!("{} is not valid JSON: {error}", path.display())))
}

/// Pretty JSON with every object's keys in order and a final newline, so
/// the same inputs always produce the same bytes, and so the same digest.
fn canonical_json(value: Value) -> Result<Vec<u8>, WindowsUnitError> {
    let mut text = serde_json::to_string_pretty(&sorted(value))
        .map_err(|error| invalid(format!("cannot encode JSON: {error}")))?;
    text.push('\n');
    Ok(text.into_bytes())
}

fn sorted(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<(String, Value)> = object.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            let mut ordered = Map::new();
            for (key, value) in entries {
                ordered.insert(key, sorted(value));
            }
            Value::Object(ordered)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

fn invalid(message: String) -> WindowsUnitError {
    WindowsUnitError::InvalidSources(message)
}

fn io_error(operation: &'static str, path: &Path, source: io::Error) -> WindowsUnitError {
    WindowsUnitError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
#[path = "unit_tests.rs"]
mod tests;
