//! Bundle units built from scratch, with synthetic signed thin Mach-O
//! executables.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use serde_json::json;
use sha2::{Digest as _, Sha256};

use crate::UnitId;

const CSMAGIC_EMBEDDED_SIGNATURE: u32 = 0xfade_0cc0;
const CSMAGIC_CODEDIRECTORY: u32 = 0xfade_0c02;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const LC_CODE_SIGNATURE: u32 = 0x1d;

pub(super) const EXECUTABLES: [&str; 3] = ["hypercolor-app", "hypercolor-daemon", "hypercolor"];

/// A thin Mach-O for this host's architecture carrying one SHA-256
/// CodeDirectory, so its cdhash is a function of `seed`.
pub(super) fn thin_macho(seed: &[u8]) -> (Vec<u8>, String) {
    let mut code_directory = vec![0_u8; 40];
    code_directory[0..4].copy_from_slice(&CSMAGIC_CODEDIRECTORY.to_be_bytes());
    code_directory.extend_from_slice(seed);
    let directory_length = u32::try_from(code_directory.len()).expect("small directory");
    code_directory[4..8].copy_from_slice(&directory_length.to_be_bytes());
    code_directory[36] = 32;
    code_directory[37] = 2;
    let mut signature = vec![0_u8; 20];
    signature[0..4].copy_from_slice(&CSMAGIC_EMBEDDED_SIGNATURE.to_be_bytes());
    signature[4..8].copy_from_slice(&(20 + directory_length).to_be_bytes());
    signature[8..12].copy_from_slice(&1_u32.to_be_bytes());
    signature[16..20].copy_from_slice(&20_u32.to_be_bytes());
    signature.extend_from_slice(&code_directory);
    let cpu_type: u32 = match std::env::consts::ARCH {
        "aarch64" => 0x0100_000c,
        "x86_64" => 0x0100_0007,
        other => panic!("no Mach-O fixture for {other}"),
    };
    let mut macho = vec![0_u8; 48];
    macho[0..4].copy_from_slice(&MH_MAGIC_64.to_le_bytes());
    macho[4..8].copy_from_slice(&cpu_type.to_le_bytes());
    macho[16..20].copy_from_slice(&1_u32.to_le_bytes());
    macho[20..24].copy_from_slice(&16_u32.to_le_bytes());
    macho[32..36].copy_from_slice(&LC_CODE_SIGNATURE.to_le_bytes());
    macho[36..40].copy_from_slice(&16_u32.to_le_bytes());
    macho[40..44].copy_from_slice(&48_u32.to_le_bytes());
    let signature_length = u32::try_from(signature.len()).expect("small signature");
    macho[44..48].copy_from_slice(&signature_length.to_le_bytes());
    macho.extend_from_slice(&signature);
    let digest = Sha256::digest(&code_directory);
    (macho, hex::encode(&digest[..20]))
}

pub(super) fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// One extracted bundle unit's tree, ready to describe and stage.
pub(super) struct UnitTree {
    pub(super) directories: Vec<(String, u32)>,
    pub(super) files: Vec<(String, u32, Vec<u8>)>,
    pub(super) version: String,
    pub(super) platform: (&'static str, &'static str),
}

impl UnitTree {
    /// A bundle whose three executables are seeded by `seed`, and whose
    /// daemon is seeded by `daemon_seed`.
    pub(super) fn new(seed: &str, daemon_seed: &str, version: &str) -> Self {
        let directories = [
            "share",
            "share/hypercolor",
            "Hypercolor.app",
            "Hypercolor.app/Contents",
            "Hypercolor.app/Contents/MacOS",
            "Hypercolor.app/Contents/Resources",
            "Hypercolor.app/Contents/_CodeSignature",
        ]
        .into_iter()
        .map(|path| (path.to_owned(), 0o755))
        .collect();
        let mut files = vec![
            (
                "share/hypercolor/durable-stores.json".to_owned(),
                0o644,
                br#"{"stores":[]}"#.to_vec(),
            ),
            (
                "Hypercolor.app/Contents/Info.plist".to_owned(),
                0o644,
                format!("<plist>{version}</plist>").into_bytes(),
            ),
            (
                "Hypercolor.app/Contents/Resources/icon.icns".to_owned(),
                0o644,
                format!("icon {seed}").into_bytes(),
            ),
            (
                "Hypercolor.app/Contents/_CodeSignature/CodeResources".to_owned(),
                0o644,
                format!("seal {seed}").into_bytes(),
            ),
        ];
        for executable in EXECUTABLES {
            let executable_seed = if executable == "hypercolor-daemon" {
                daemon_seed
            } else {
                seed
            };
            files.push((
                format!("Hypercolor.app/Contents/MacOS/{executable}"),
                0o755,
                thin_macho(format!("{executable_seed}:{executable}").as_bytes()).0,
            ));
        }
        Self {
            directories,
            files,
            version: version.to_owned(),
            platform: ("macos-aarch64", "aarch64-apple-darwin"),
        }
    }

    pub(super) fn manifest(&self) -> Vec<u8> {
        let mut members = self
            .directories
            .iter()
            .map(|(path, mode)| json!({"path": path, "type": "directory", "mode": mode}))
            .chain(self.files.iter().map(|(path, mode, bytes)| {
                json!({
                    "path": path,
                    "type": "file",
                    "mode": mode,
                    "size": bytes.len(),
                    "sha256": sha256(bytes),
                })
            }))
            .collect::<Vec<_>>();
        members.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
        serde_json::to_vec_pretty(&json!({
            "name": "hypercolor",
            "version": self.version,
            "platform": self.platform.0,
            "rust_target": self.platform.1,
            "members": members,
        }))
        .expect("encode bundle manifest")
    }

    /// Write the tree and its manifest under `root`, returning the digest.
    pub(super) fn write(&self, root: &Path) -> UnitId {
        self.write_with_manifest(root, &self.manifest())
    }

    pub(super) fn write_with_manifest(&self, root: &Path, manifest: &[u8]) -> UnitId {
        for (path, _) in &self.directories {
            fs::create_dir_all(root.join(path)).expect("create unit directory");
        }
        for (path, mode, bytes) in &self.files {
            fs::write(root.join(path), bytes).expect("write unit file");
            fs::set_permissions(root.join(path), fs::Permissions::from_mode(*mode))
                .expect("set unit file mode");
        }
        // Deepest first, so a read-only directory never blocks its children.
        for (path, mode) in self.directories.iter().rev() {
            fs::set_permissions(root.join(path), fs::Permissions::from_mode(*mode))
                .expect("set unit directory mode");
        }
        fs::write(root.join("manifest.json"), manifest).expect("write manifest");
        fs::set_permissions(
            root.join("manifest.json"),
            fs::Permissions::from_mode(0o644),
        )
        .expect("set manifest mode");
        UnitId::new(sha256(manifest)).expect("manifest digest")
    }

    pub(super) fn installed_bytes(&self) -> u64 {
        let files: usize = self.files.iter().map(|(_, _, bytes)| bytes.len()).sum();
        (self.manifest().len() + files) as u64
    }
}
