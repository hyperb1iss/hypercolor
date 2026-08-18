#[cfg(not(target_os = "macos"))]
use hypercolor_macos_input::MacosInputError;
use hypercolor_macos_input::current_process_audit_token_identity;

#[test]
fn audit_token_identity_is_platform_explicit_and_bounded() {
    #[cfg(target_os = "macos")]
    {
        let identity = current_process_audit_token_identity()
            .expect("current macOS process exposes an audit token");
        let words = identity.split(':').collect::<Vec<_>>();
        assert_eq!(words.len(), 8);
        assert!(
            words.iter().all(|word| {
                word.len() == 8 && word.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        );
    }

    #[cfg(not(target_os = "macos"))]
    assert_eq!(
        current_process_audit_token_identity(),
        Err(MacosInputError::UnsupportedPlatform)
    );
}
