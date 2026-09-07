use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest as _, Sha256};

use super::{AcceptedImageProof, validate_retained_with};
use crate::{MacosDirectLaunchdExecutableExpectation, MacosOwnerExecutionError};

const REQUIREMENT: &str = "identifier \"com.apple.ls\" and anchor apple";
const CDHASH: &str = "0123456789abcdef0123456789abcdef01234567";

struct SwappingProof {
    path: PathBuf,
    replacement: &'static str,
    observed_cdhash: String,
}

impl AcceptedImageProof for SwappingProof {
    fn prove(
        &mut self,
        path: &Path,
        _designated_requirement: &str,
        cdhash: &str,
        _deadline: Instant,
    ) -> Result<bool, MacosOwnerExecutionError> {
        assert_eq!(path, self.path);
        fs::remove_file(&self.path).expect("original path should remove at proof boundary");
        fs::copy(self.replacement, &self.path).expect("replacement image should copy");
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o555))
            .expect("replacement mode should set");
        Ok(self.observed_cdhash == cdhash)
    }
}

#[test]
fn accepted_image_boundary_rejects_different_path_image() {
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let path = directory.path().join("hypercolor-daemon");
    fs::copy("/bin/ls", &path).expect("fixture should copy");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o555)).expect("fixture mode should set");
    let retained = fs::File::open(&path).expect("fixture should retain");
    let metadata = retained.metadata().expect("fixture metadata should read");
    let bytes = fs::read(&path).expect("fixture bytes should read");
    let expected = MacosDirectLaunchdExecutableExpectation::new(
        &path,
        REQUIREMENT,
        hex(&Sha256::digest(REQUIREMENT.as_bytes())),
        CDHASH,
        hex(&Sha256::digest(&bytes)),
        metadata.mode() & 0o7777,
        metadata.len(),
        metadata.dev(),
        metadata.ino(),
    )
    .expect("expectation should build");
    let mut proof = SwappingProof {
        path,
        replacement: "/bin/cat",
        observed_cdhash: "89abcdef0123456789abcdef0123456789abcdef".to_owned(),
    };
    assert!(
        !validate_retained_with(
            &retained,
            &expected,
            Instant::now() + Duration::from_secs(2),
            &mut proof,
        )
        .expect("boundary swap should produce an exact mismatch")
    );
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes.iter().fold(
        String::with_capacity(bytes.len() * 2),
        |mut output, byte| {
            write!(&mut output, "{byte:02x}").expect("hex should write");
            output
        },
    )
}
