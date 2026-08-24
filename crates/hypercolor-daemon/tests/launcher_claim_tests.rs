use std::ffi::OsStr;

use hypercolor_daemon::launcher_claim::{
    LinuxLauncherEvidence, ensure_claims_agree, parse_systemd_main_pid,
    read_service_identity_claim, resolve_launcher_identity, resolve_linux_launcher_identity,
    same_launcher,
};
use hypercolor_types::service::ServiceIdentity;

#[test]
fn claim_reader_accepts_absent_and_well_formed_declarations() {
    assert_eq!(
        read_service_identity_claim(None).expect("absent claim is fine"),
        None
    );
    assert_eq!(
        read_service_identity_claim(Some(OsStr::new("user_service:systemd:hypercolor.service")))
            .expect("declaration parses"),
        Some(ServiceIdentity::systemd_user())
    );
}

#[test]
fn claim_reader_rejects_malformed_declarations() {
    assert!(read_service_identity_claim(Some(OsStr::new(""))).is_err());
    assert!(read_service_identity_claim(Some(OsStr::new("systemd"))).is_err());
    assert!(read_service_identity_claim(Some(OsStr::new("standalone:launchd"))).is_err());
}

#[cfg(unix)]
#[test]
fn claim_reader_rejects_non_utf8() {
    use std::os::unix::ffi::OsStrExt;
    assert!(read_service_identity_claim(Some(OsStr::from_bytes(&[0xff]))).is_err());
}

#[test]
fn unit_label_is_diagnostic_for_corroboration() {
    let claimed = ServiceIdentity {
        unit: Some("renamed.service".into()),
        ..ServiceIdentity::systemd_user()
    };
    assert!(same_launcher(&claimed, &ServiceIdentity::systemd_user()));
    assert!(!same_launcher(
        &ServiceIdentity::systemd_user(),
        &ServiceIdentity::systemd_system()
    ));
    assert!(!same_launcher(
        &ServiceIdentity::launchd_direct(),
        &ServiceIdentity::homebrew()
    ));
}

#[test]
fn resolution_returns_the_measured_authority_not_the_claim() {
    let claimed = ServiceIdentity {
        unit: Some("renamed.service".into()),
        ..ServiceIdentity::systemd_user()
    };
    let resolved = resolve_launcher_identity(Some(&claimed), &[ServiceIdentity::systemd_user()])
        .expect("matching claim resolves");
    assert_eq!(resolved, ServiceIdentity::systemd_user());
}

#[test]
fn absent_metadata_resolves_through_bounded_inference() {
    assert_eq!(
        resolve_launcher_identity(None, &[]).expect("residual"),
        ServiceIdentity::STANDALONE
    );
    assert_eq!(
        resolve_launcher_identity(None, &[ServiceIdentity::windows_scm()]).expect("scm"),
        ServiceIdentity::windows_scm()
    );
    assert_eq!(
        resolve_launcher_identity(None, &[ServiceIdentity::APP_SIDECAR]).expect("sidecar"),
        ServiceIdentity::APP_SIDECAR
    );
}

#[test]
fn contradicted_and_ambiguous_claims_fail_closed() {
    assert!(resolve_launcher_identity(Some(&ServiceIdentity::systemd_user()), &[]).is_err());
    assert!(
        resolve_launcher_identity(
            Some(&ServiceIdentity::STANDALONE),
            &[ServiceIdentity::windows_scm()]
        )
        .is_err()
    );
    assert!(
        resolve_launcher_identity(
            None,
            &[
                ServiceIdentity::APP_SIDECAR,
                ServiceIdentity::systemd_user()
            ]
        )
        .is_err()
    );
}

#[test]
fn neutral_and_legacy_claims_must_agree_when_both_present() {
    ensure_claims_agree(None, None).expect("nothing to compare");
    ensure_claims_agree(Some(&ServiceIdentity::homebrew()), None).expect("one claim");
    ensure_claims_agree(
        Some(&ServiceIdentity::homebrew()),
        Some(&ServiceIdentity::homebrew()),
    )
    .expect("equal claims");
    assert!(
        ensure_claims_agree(
            Some(&ServiceIdentity::homebrew()),
            Some(&ServiceIdentity::launchd_direct()),
        )
        .is_err()
    );
}

#[test]
fn linux_evidence_resolves_every_launcher_deterministically() {
    let cases = [
        (
            LinuxLauncherEvidence::default(),
            ServiceIdentity::STANDALONE,
        ),
        (
            LinuxLauncherEvidence {
                supervised_child: true,
                ..LinuxLauncherEvidence::default()
            },
            ServiceIdentity::APP_SIDECAR,
        ),
        (
            LinuxLauncherEvidence {
                systemd_user: true,
                ..LinuxLauncherEvidence::default()
            },
            ServiceIdentity::systemd_user(),
        ),
        (
            LinuxLauncherEvidence {
                systemd_system: true,
                ..LinuxLauncherEvidence::default()
            },
            ServiceIdentity::systemd_system(),
        ),
    ];
    for (evidence, expected) in cases {
        assert_eq!(
            resolve_linux_launcher_identity(None, evidence).expect("inference"),
            expected,
            "{evidence:?}"
        );
        assert_eq!(
            resolve_linux_launcher_identity(Some(&expected), evidence).expect("corroborated"),
            expected,
            "{evidence:?}"
        );
    }
    assert!(
        resolve_linux_launcher_identity(
            Some(&ServiceIdentity::systemd_user()),
            LinuxLauncherEvidence {
                systemd_system: true,
                ..LinuxLauncherEvidence::default()
            },
        )
        .is_err()
    );
    assert!(
        resolve_linux_launcher_identity(
            None,
            LinuxLauncherEvidence {
                systemd_user: true,
                systemd_system: true,
                supervised_child: false,
            },
        )
        .is_err()
    );
}

#[test]
fn systemd_main_pid_parser_accepts_value_and_property_forms() {
    assert_eq!(parse_systemd_main_pid("4242\n"), Some(4242));
    assert_eq!(parse_systemd_main_pid("MainPID=4242\n"), Some(4242));
    assert_eq!(parse_systemd_main_pid("\n  17 \n"), Some(17));
    assert_eq!(parse_systemd_main_pid("0\n"), None);
    assert_eq!(parse_systemd_main_pid("MainPID=0"), None);
    assert_eq!(parse_systemd_main_pid(""), None);
    assert_eq!(parse_systemd_main_pid("not a pid"), None);
}
