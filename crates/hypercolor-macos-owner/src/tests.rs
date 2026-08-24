use crate::store_io::validate_private_file_uid;

#[test]
fn private_session_file_rejects_another_uid() {
    let error = validate_private_file_uid(501, 502, "daemon session attestation")
        .expect_err("another UID must fail closed");

    assert!(error.to_string().contains("current user"));
}
