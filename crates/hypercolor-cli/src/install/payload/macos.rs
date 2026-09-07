use std::fmt::Write as _;
use std::io::Read as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use security_framework::os::macos::code_signing::SecRequirement;
use serde::de::{Error as _, IgnoredAny, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use sha2::{Digest as _, Sha256};

use super::manifest::{MAX_RELEASE_PATH_BYTES, ValidatedManifest, ValidatedMember};
use super::{ReleasePayloadError, tree};
use crate::install::UnitRecord;

const PROVENANCE_PATH: &str = "share/hypercolor/macos-notarization.json";
const DAEMON_PATH: &str = "bin/hypercolor-daemon";
const DAEMON_IDENTIFIER: &str = "tech.hyperbliss.hypercolor.daemon";
const DEVELOPER_ID_INTERMEDIATE_CLAUSE: &str =
    "certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */";
const DEVELOPER_ID_APPLICATION_CLAUSE: &str =
    "certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */";
const MAX_PROVENANCE_BYTES: u64 = 1024 * 1024;
const MAX_SIGNED_OBJECTS: usize = 128;
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_REQUIREMENT_BYTES: usize = 8 * 1024;
const MAX_NOTARIZATION_MESSAGE_BYTES: usize = 1024;
const MAX_TARGET_BYTES: usize = 128;
const NOTARIZATION_ID_BYTES: usize = 36;
const NOTARIZATION_STATUS_BYTES: usize = 32;
const TEAM_ID_BYTES: usize = 10;
const CDHASH_HEX_BYTES: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosReleaseProvenance {
    daemon_sha256: String,
    daemon_size: u64,
    daemon_mode: u32,
    daemon_device: u64,
    daemon_inode: u64,
    designated_requirement: String,
    designated_requirement_sha256: String,
    cdhash: String,
    team_id: String,
}

impl MacosReleaseProvenance {
    #[must_use]
    pub fn daemon_sha256(&self) -> &str {
        &self.daemon_sha256
    }

    #[must_use]
    pub const fn daemon_size(&self) -> u64 {
        self.daemon_size
    }

    #[must_use]
    pub const fn daemon_mode(&self) -> u32 {
        self.daemon_mode
    }

    #[must_use]
    pub const fn daemon_device(&self) -> u64 {
        self.daemon_device
    }

    #[must_use]
    pub const fn daemon_inode(&self) -> u64 {
        self.daemon_inode
    }

    #[must_use]
    pub fn designated_requirement(&self) -> &str {
        &self.designated_requirement
    }

    #[must_use]
    pub fn designated_requirement_sha256(&self) -> &str {
        &self.designated_requirement_sha256
    }

    #[must_use]
    pub fn cdhash(&self) -> &str {
        &self.cdhash
    }

    #[must_use]
    pub fn team_id(&self) -> &str {
        &self.team_id
    }
}

pub fn bind_macos_release_provenance(
    unit: &UnitRecord,
) -> Result<MacosReleaseProvenance, ReleasePayloadError> {
    let manifest_bytes = tree::read_retained_manifest_bytes(unit.directory())?;
    let manifest = ValidatedManifest::parse(manifest_bytes)?;
    if manifest.unit_id != *unit.id() {
        return Err(ReleasePayloadError::UnexpectedManifestDigest {
            expected: unit.id().as_str().to_owned(),
            actual: manifest.unit_id.as_str().to_owned(),
        });
    }
    require_native_identity(&manifest)?;
    tree::validate_retained(unit.directory(), &manifest)?;

    let provenance_bytes = read_member(unit, &manifest, PROVENANCE_PATH, MAX_PROVENANCE_BYTES)?;
    let provenance: RawProvenance =
        serde_json::from_slice(&provenance_bytes).map_err(|source| {
            ReleasePayloadError::InvalidUnit(format!(
                "macOS notarization provenance is not strict JSON: {source}"
            ))
        })?;
    provenance.bind(unit, &manifest)
}

fn require_native_identity(manifest: &ValidatedManifest) -> Result<(), ReleasePayloadError> {
    let (platform, rust_target) = match std::env::consts::ARCH {
        "aarch64" => ("macos-arm64", "aarch64-apple-darwin"),
        "x86_64" => ("macos-amd64", "x86_64-apple-darwin"),
        architecture => {
            return Err(ReleasePayloadError::InvalidUnit(format!(
                "unsupported macOS release architecture {architecture}"
            )));
        }
    };
    if manifest.platform != platform || manifest.rust_target != rust_target {
        return Err(ReleasePayloadError::InvalidUnit(
            "release platform identity does not match this macOS process".to_owned(),
        ));
    }
    Ok(())
}

fn read_member(
    unit: &UnitRecord,
    manifest: &ValidatedManifest,
    path: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, ReleasePayloadError> {
    let ValidatedMember::File {
        source_mode,
        size,
        sha256,
    } = manifest
        .members
        .get(path)
        .ok_or_else(|| ReleasePayloadError::InvalidUnit(format!("missing {path}")))?
    else {
        return Err(ReleasePayloadError::InvalidUnit(format!(
            "{path} is not a regular file"
        )));
    };
    if *size > max_bytes {
        return Err(ReleasePayloadError::InvalidUnit(format!(
            "{path} exceeds its {max_bytes}-byte bound"
        )));
    }
    let mut opened = open_member(unit, path)?;
    let metadata = opened.metadata();
    if metadata.mode() != source_mode & !0o222 || metadata.size() != *size {
        return Err(ReleasePayloadError::InvalidUnit(format!(
            "installed metadata mismatch for {path}"
        )));
    }
    let capacity = usize::try_from(*size)
        .map_err(|_| ReleasePayloadError::InvalidUnit(format!("{path} does not fit in memory")))?;
    let mut bytes = Vec::with_capacity(capacity);
    opened
        .file_mut()
        .take(*size + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "read a retained macOS release member",
            source,
        })?;
    if bytes.len() != capacity || hex_digest(&bytes) != *sha256 {
        return Err(ReleasePayloadError::InvalidUnit(format!(
            "installed content mismatch for {path}"
        )));
    }
    let after = opened
        .file()
        .metadata()
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "reinspect a retained macOS release member",
            source,
        })?;
    if !retained_metadata_matches(metadata, &after) {
        return Err(ReleasePayloadError::InvalidUnit(format!(
            "installed identity drift for {path}"
        )));
    }
    Ok(bytes)
}

fn open_member(
    unit: &UnitRecord,
    path: &str,
) -> Result<hypercolor_platform_fs::OpenedRegularFile, ReleasePayloadError> {
    let (parent, name) = path
        .rsplit_once('/')
        .ok_or_else(|| ReleasePayloadError::InvalidUnit(format!("invalid member path {path}")))?;
    let mut directory: Option<hypercolor_platform_fs::ReadOnlyDirectoryAuthority> = None;
    for component in parent.split('/') {
        let child = match directory.as_ref() {
            Some(directory) => directory.open_child_directory(Path::new(component)),
            None => unit.directory().open_child_directory(Path::new(component)),
        }
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "open a retained macOS release directory",
            source,
        })?;
        directory = Some(child);
    }
    directory
        .as_ref()
        .expect("macOS provenance members have a parent directory")
        .open_regular_file(Path::new(name))
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "open a retained macOS release member",
            source,
        })
}

fn retained_metadata_matches(
    before: hypercolor_platform_fs::DirectoryEntryMetadata,
    after: &std::fs::Metadata,
) -> bool {
    after.file_type().is_file()
        && after.mode() & 0o7777 == before.mode()
        && after.len() == before.size()
        && after.nlink() == before.link_count()
        && after.dev() == before.device()
        && after.ino() == before.inode()
}

fn bind_retained_file_identity(
    unit: &UnitRecord,
    manifest: &ValidatedManifest,
    path: &str,
) -> Result<(hypercolor_platform_fs::DirectoryEntryMetadata, String), ReleasePayloadError> {
    let ValidatedMember::File {
        source_mode,
        size,
        sha256,
    } = manifest
        .members
        .get(path)
        .ok_or_else(|| ReleasePayloadError::InvalidUnit(format!("missing {path}")))?
    else {
        return Err(ReleasePayloadError::InvalidUnit(format!(
            "{path} is not a regular file"
        )));
    };
    let mut opened = open_member(unit, path)?;
    let metadata = opened.metadata();
    if metadata.mode() != source_mode & !0o222
        || metadata.size() != *size
        || metadata.link_count() != 1
    {
        return Err(ReleasePayloadError::InvalidUnit(format!(
            "installed metadata mismatch for {path}"
        )));
    }
    let limit = size.checked_add(1).ok_or_else(|| {
        ReleasePayloadError::InvalidUnit(format!("{path} exceeds the retained file size bound"))
    })?;
    let mut hasher = Sha256::new();
    let mut read_total = 0_u64;
    let mut buffer = [0_u8; 8 * 1024];
    let mut limited = opened.file_mut().take(limit);
    loop {
        let read = limited
            .read(&mut buffer)
            .map_err(|source| ReleasePayloadError::Filesystem {
                operation: "read a retained macOS release executable",
                source,
            })?;
        if read == 0 {
            break;
        }
        read_total += u64::try_from(read).expect("read length fits u64");
        hasher.update(&buffer[..read]);
    }
    if read_total != *size || hex_encoded_digest(hasher.finalize().as_slice()) != *sha256 {
        return Err(ReleasePayloadError::InvalidUnit(format!(
            "installed content mismatch for {path}"
        )));
    }
    let cdhash = crate::install::macos::thin_macho_cdhash(opened.file_mut(), *size)
        .map_err(|source| ReleasePayloadError::InvalidUnit(source.to_string()))?;
    let after = opened
        .file()
        .metadata()
        .map_err(|source| ReleasePayloadError::Filesystem {
            operation: "reinspect a retained macOS release executable",
            source,
        })?;
    if !retained_metadata_matches(metadata, &after) {
        return Err(ReleasePayloadError::InvalidUnit(format!(
            "installed identity drift for {path}"
        )));
    }
    Ok((metadata, cdhash))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProvenance {
    #[serde(deserialize_with = "deserialize_team_id")]
    team_id: String,
    #[serde(deserialize_with = "deserialize_target")]
    target: String,
    #[serde(deserialize_with = "deserialize_signed_objects")]
    objects: Vec<RawSignedObject>,
    notarization: RawNotarization,
}

impl RawProvenance {
    fn bind(
        self,
        unit: &UnitRecord,
        manifest: &ValidatedManifest,
    ) -> Result<MacosReleaseProvenance, ReleasePayloadError> {
        if self.target != manifest.rust_target
            || self.team_id.len() != TEAM_ID_BYTES
            || !self
                .team_id
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
            || self.notarization.status != "Accepted"
            || !valid_notarization_id(&self.notarization.id)
            || self.notarization.message.is_empty()
            || self.notarization.message.len() > MAX_NOTARIZATION_MESSAGE_BYTES
            || self
                .notarization
                .message
                .bytes()
                .any(|byte| byte.is_ascii_control())
            || self.objects.is_empty()
        {
            return Err(invalid_provenance());
        }
        let mut daemon = None;
        let mut paths = std::collections::BTreeSet::new();
        for object in self.objects {
            if !paths.insert(object.path.clone())
                || !manifest.members.contains_key(&object.path)
                || object.identifier.is_empty()
                || object.identifier.len() > MAX_IDENTIFIER_BYTES
            {
                return Err(invalid_provenance());
            }
            if object.path == DAEMON_PATH
                && (object.identifier != DAEMON_IDENTIFIER || daemon.replace(object).is_some())
            {
                return Err(invalid_provenance());
            }
        }
        let daemon = daemon.ok_or_else(invalid_provenance)?;
        let ValidatedMember::File {
            source_mode,
            size,
            sha256,
        } = manifest
            .members
            .get(DAEMON_PATH)
            .ok_or_else(invalid_provenance)?
        else {
            return Err(invalid_provenance());
        };
        if *source_mode != 0o755 {
            return Err(invalid_provenance());
        }
        let (daemon_metadata, retained_cdhash) =
            bind_retained_file_identity(unit, manifest, DAEMON_PATH)?;
        if daemon.cdhash != retained_cdhash {
            return Err(invalid_provenance());
        }
        let designated_requirement = bind_designated_requirement(
            &daemon.designated_requirement,
            DAEMON_IDENTIFIER,
            &self.team_id,
        )?;
        Ok(MacosReleaseProvenance {
            daemon_sha256: sha256.clone(),
            daemon_size: *size,
            daemon_mode: daemon_metadata.mode(),
            daemon_device: daemon_metadata.device(),
            daemon_inode: daemon_metadata.inode(),
            designated_requirement_sha256: hex_digest(designated_requirement.as_bytes()),
            designated_requirement,
            cdhash: daemon.cdhash,
            team_id: self.team_id,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSignedObject {
    #[serde(deserialize_with = "deserialize_object_path")]
    path: String,
    #[serde(deserialize_with = "deserialize_identifier")]
    identifier: String,
    #[serde(deserialize_with = "deserialize_requirement")]
    designated_requirement: String,
    #[serde(deserialize_with = "deserialize_cdhash")]
    cdhash: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNotarization {
    #[serde(deserialize_with = "deserialize_notarization_id")]
    id: String,
    #[serde(deserialize_with = "deserialize_notarization_message")]
    message: String,
    #[serde(deserialize_with = "deserialize_notarization_status")]
    status: String,
}

fn deserialize_signed_objects<'de, D>(deserializer: D) -> Result<Vec<RawSignedObject>, D::Error>
where
    D: Deserializer<'de>,
{
    struct SignedObjectsVisitor;

    impl<'de> Visitor<'de> for SignedObjectsVisitor {
        type Value = Vec<RawSignedObject>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "at most {MAX_SIGNED_OBJECTS} signed objects")
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut objects = Vec::with_capacity(
                sequence
                    .size_hint()
                    .unwrap_or_default()
                    .min(MAX_SIGNED_OBJECTS),
            );
            while objects.len() < MAX_SIGNED_OBJECTS {
                let Some(object) = sequence.next_element()? else {
                    return Ok(objects);
                };
                objects.push(object);
            }
            if sequence.next_element::<IgnoredAny>()?.is_some() {
                return Err(A::Error::custom(format_args!(
                    "signed object count exceeds {MAX_SIGNED_OBJECTS}"
                )));
            }
            Ok(objects)
        }
    }

    deserializer.deserialize_seq(SignedObjectsVisitor)
}

fn deserialize_object_path<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_bounded_string::<D, MAX_RELEASE_PATH_BYTES>(deserializer, "signed object path")
}

fn deserialize_identifier<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_bounded_string::<D, MAX_IDENTIFIER_BYTES>(deserializer, "signed object identifier")
}

fn deserialize_requirement<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_bounded_string::<D, MAX_REQUIREMENT_BYTES>(deserializer, "designated requirement")
}

fn deserialize_cdhash<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = deserialize_bounded_string::<D, CDHASH_HEX_BYTES>(deserializer, "CDHash")?;
    if value.len() != CDHASH_HEX_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(D::Error::custom(
            "CDHash must be exactly 40 lowercase hexadecimal bytes",
        ));
    }
    Ok(value)
}

fn deserialize_team_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_bounded_string::<D, TEAM_ID_BYTES>(deserializer, "team ID")
}

fn deserialize_target<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_bounded_string::<D, MAX_TARGET_BYTES>(deserializer, "notarization target")
}

fn deserialize_notarization_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_bounded_string::<D, NOTARIZATION_ID_BYTES>(deserializer, "notarization ID")
}

fn deserialize_notarization_message<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_bounded_string::<D, MAX_NOTARIZATION_MESSAGE_BYTES>(
        deserializer,
        "notarization message",
    )
}

fn deserialize_notarization_status<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_bounded_string::<D, NOTARIZATION_STATUS_BYTES>(deserializer, "notarization status")
}

fn deserialize_bounded_string<'de, D, const MAX: usize>(
    deserializer: D,
    field: &'static str,
) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    struct BoundedStringVisitor<const MAX: usize> {
        field: &'static str,
    }

    impl<const MAX: usize> BoundedStringVisitor<MAX> {
        fn bind<E: serde::de::Error>(self, value: &str) -> Result<String, E> {
            if value.len() > MAX {
                return Err(E::custom(format_args!(
                    "{} exceeds its {MAX}-byte bound",
                    self.field
                )));
            }
            Ok(value.to_owned())
        }
    }

    impl<'de, const MAX: usize> Visitor<'de> for BoundedStringVisitor<MAX> {
        type Value = String;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "{} no longer than {MAX} bytes", self.field)
        }

        fn visit_borrowed_str<E>(self, value: &'de str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            self.bind(value)
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            self.bind(value)
        }

        fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            if value.len() > MAX {
                return Err(E::custom(format_args!(
                    "{} exceeds its {MAX}-byte bound",
                    self.field
                )));
            }
            Ok(value)
        }
    }

    deserializer.deserialize_string(BoundedStringVisitor::<MAX> { field })
}

fn bind_designated_requirement(
    raw: &str,
    identifier: &str,
    team_id: &str,
) -> Result<String, ReleasePayloadError> {
    let requirement = raw.strip_prefix("designated => ").unwrap_or(raw);
    if requirement.is_empty()
        || requirement.len() > MAX_REQUIREMENT_BYTES
        || requirement
            .bytes()
            .any(|byte| byte.is_ascii_control() || !byte.is_ascii())
    {
        return Err(invalid_provenance());
    }
    let _: SecRequirement = requirement.parse().map_err(|_| invalid_provenance())?;
    let expected = format!(
        "identifier \"{identifier}\" and anchor apple generic and \
         {DEVELOPER_ID_INTERMEDIATE_CLAUSE} and {DEVELOPER_ID_APPLICATION_CLAUSE} and \
         certificate leaf[subject.OU] = \"{team_id}\""
    );
    if requirement != expected {
        return Err(invalid_provenance());
    }
    Ok(requirement.to_owned())
}

fn valid_notarization_id(value: &str) -> bool {
    value.len() == NOTARIZATION_ID_BYTES
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn invalid_provenance() -> ReleasePayloadError {
    ReleasePayloadError::InvalidUnit(
        "macOS notarization provenance does not bind the retained daemon".to_owned(),
    )
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex_encoded_digest(&digest)
}

fn hex_encoded_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(64);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("writing into a String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    use hypercolor_platform_fs::ReadOnlyDirectoryAuthority;

    use super::retained_metadata_matches;

    #[test]
    fn retained_metadata_rejects_permission_and_special_bit_drift() {
        let directory = tempfile::tempdir().expect("temporary retained directory");
        let path = directory.path().join("member");
        fs::write(&path, b"member").expect("write retained member");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444))
            .expect("set retained member mode");
        let authority = ReadOnlyDirectoryAuthority::open(directory.path())
            .expect("open retained directory authority");
        let opened = authority
            .open_regular_file(Path::new("member"))
            .expect("open retained member");
        let before = opened.metadata();

        for mode in [0o644, 0o444 | 0o4000] {
            fs::set_permissions(&path, fs::Permissions::from_mode(mode))
                .expect("mutate retained member mode");
            let after = opened.file().metadata().expect("reinspect retained member");
            assert!(!retained_metadata_matches(before, &after));
        }
    }
}
