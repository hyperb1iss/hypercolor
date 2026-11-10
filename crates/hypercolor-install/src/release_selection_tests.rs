use std::collections::VecDeque;
use std::io;

use super::{ReleaseRecords, ReleaseSelectionError, select_app_release, select_release};
use crate::model::UnitId;

fn unit(byte: char) -> UnitId {
    UnitId::new(byte.to_string().repeat(64)).expect("unit")
}

fn journal(disposition: &str, prior: Option<&UnitId>, candidate: &UnitId) -> Vec<u8> {
    serde_json::json!({
        "schema_version": 3,
        "revision": 7,
        "disposition": disposition,
        "prior_active_unit": prior,
        "candidate_unit": candidate,
        "platform_record": { "platform": "a platform this build never heard of" },
    })
    .to_string()
    .into_bytes()
}

/// A journal resting before `action`.
fn journal_at(
    disposition: &str,
    action: &str,
    prior: Option<&UnitId>,
    candidate: &UnitId,
) -> Vec<u8> {
    let mut journal: serde_json::Value =
        serde_json::from_slice(&journal(disposition, prior, candidate)).expect("journal");
    journal["next_action"] = serde_json::Value::from(action);
    journal.to_string().into_bytes()
}

fn pointer(unit: &UnitId) -> Vec<u8> {
    super::encode_active_pointer(unit).expect("pointer")
}

/// Records that answer each read from a script, so a test can change them
/// between two reads the way a running transaction does.
#[derive(Default)]
struct Scripted {
    journals: VecDeque<Option<Vec<u8>>>,
    pointers: VecDeque<Option<Vec<u8>>>,
    reads: Vec<&'static str>,
}

impl ReleaseRecords for Scripted {
    fn read_journal(&mut self) -> io::Result<Option<Vec<u8>>> {
        self.reads.push("journal");
        Ok(self.journals.pop_front().expect("scripted journal"))
    }

    fn read_pointer(&mut self) -> io::Result<Option<Vec<u8>>> {
        self.reads.push("pointer");
        Ok(self.pointers.pop_front().expect("scripted pointer"))
    }
}

fn scripted(journals: Vec<Option<Vec<u8>>>, pointers: Vec<Option<Vec<u8>>>) -> Scripted {
    Scripted {
        journals: journals.into(),
        pointers: pointers.into(),
        reads: Vec::new(),
    }
}

#[test]
fn with_no_transaction_the_active_unit_runs() {
    let active = unit('a');
    let mut records = scripted(vec![None, None], vec![Some(pointer(&active))]);

    assert_eq!(select_release(&mut records).expect("select"), active);
    assert_eq!(records.reads, ["journal", "pointer", "journal"]);
}

#[test]
fn a_terminal_journal_leaves_the_choice_to_the_pointer() {
    let prior = unit('a');
    let candidate = unit('b');
    for disposition in ["committed", "rolled_back"] {
        let bytes = journal(disposition, Some(&prior), &candidate);
        let mut records = scripted(
            vec![Some(bytes.clone()), Some(bytes)],
            vec![Some(pointer(&candidate))],
        );

        assert_eq!(
            select_release(&mut records).expect("select"),
            candidate,
            "{disposition}"
        );
    }
}

#[test]
fn an_open_transaction_runs_its_prior_even_when_the_pointer_names_the_candidate() {
    let prior = unit('a');
    let candidate = unit('b');
    for disposition in ["forward", "rollback"] {
        let mut records = scripted(
            vec![Some(journal(disposition, Some(&prior), &candidate))],
            vec![Some(pointer(&candidate))],
        );

        assert_eq!(
            select_release(&mut records).expect("select"),
            prior,
            "{disposition}"
        );
        assert_eq!(
            records.reads,
            ["journal"],
            "an open journal decides before the pointer is read"
        );
    }
}

#[test]
fn once_the_journal_records_the_candidate_s_proof_its_app_runs() {
    let prior = unit('a');
    let candidate = unit('b');
    let at_commit = journal_at("forward", "commit", Some(&prior), &candidate);

    let mut records = scripted(vec![Some(at_commit.clone())], vec![]);
    assert_eq!(
        select_app_release(&mut records).expect("select"),
        candidate,
        "only the commit record is left, and it names the candidate"
    );
    assert_eq!(records.reads, ["journal"]);

    let mut records = scripted(vec![Some(at_commit)], vec![]);
    assert_eq!(
        select_release(&mut records).expect("select"),
        prior,
        "recovery still runs the code that wrote the journal"
    );
}

#[test]
fn before_the_candidate_s_proof_is_recorded_its_app_never_runs() {
    let prior = unit('a');
    let candidate = unit('b');
    for (disposition, action) in [
        ("forward", "switch_to_candidate"),
        ("forward", "restore_candidate_runtime"),
        ("forward", "prove_candidate"),
        ("forward", "an_action_a_later_release_added"),
        ("rollback", "unload_candidate_runtime"),
        ("rollback", "prove_prior"),
    ] {
        let mut records = scripted(
            vec![Some(journal_at(
                disposition,
                action,
                Some(&prior),
                &candidate,
            ))],
            vec![Some(pointer(&candidate))],
        );

        assert_eq!(
            select_app_release(&mut records).expect("select"),
            prior,
            "{disposition} at {action}"
        );
    }
}

#[test]
fn a_first_install_runs_its_candidate_only_once_its_proof_is_recorded() {
    let candidate = unit('b');
    let mut records = scripted(
        vec![Some(journal_at("forward", "commit", None, &candidate))],
        vec![],
    );
    assert_eq!(select_app_release(&mut records).expect("select"), candidate);

    let mut records = scripted(
        vec![Some(journal_at(
            "forward",
            "prove_candidate",
            None,
            &candidate,
        ))],
        vec![],
    );
    assert!(matches!(
        select_app_release(&mut records),
        Err(ReleaseSelectionError::NoPrior)
    ));
}

#[test]
fn a_transaction_that_opens_after_the_journal_read_still_runs_its_prior() {
    let prior = unit('a');
    let candidate = unit('b');
    let finished = journal("committed", Some(&unit('c')), &prior);
    let open = journal("forward", Some(&prior), &candidate);
    let mut records = scripted(
        vec![Some(finished), Some(open.clone()), Some(open)],
        vec![Some(pointer(&candidate))],
    );

    assert_eq!(select_release(&mut records).expect("select"), prior);
}

#[test]
fn a_first_install_in_flight_has_nothing_proven_to_run() {
    let candidate = unit('b');
    let mut records = scripted(
        vec![Some(journal("forward", None, &candidate))],
        vec![Some(pointer(&candidate))],
    );

    assert!(matches!(
        select_release(&mut records),
        Err(ReleaseSelectionError::NoPrior)
    ));
}

#[test]
fn an_installation_without_a_pointer_has_no_active_release() {
    let mut records = scripted(vec![None, None], vec![None]);

    assert!(matches!(
        select_release(&mut records),
        Err(ReleaseSelectionError::NoActive)
    ));
}

#[test]
fn records_that_never_hold_still_are_refused() {
    let one = journal("committed", None, &unit('a'));
    let two = journal("rolled_back", None, &unit('a'));
    let mut records = scripted(
        vec![
            Some(one.clone()),
            Some(two.clone()),
            Some(one.clone()),
            Some(two.clone()),
            Some(one),
            Some(two),
        ],
        vec![None, None, None],
    );

    assert!(matches!(
        select_release(&mut records),
        Err(ReleaseSelectionError::Unsettled)
    ));
}

#[test]
fn an_unreadable_journal_is_refused_rather_than_read_as_no_transaction() {
    let mut records = scripted(vec![Some(b"{".to_vec())], vec![]);

    assert!(matches!(
        select_release(&mut records),
        Err(ReleaseSelectionError::InvalidJournal(_))
    ));
}

#[test]
fn a_journal_state_this_build_never_heard_of_is_refused() {
    let bytes = journal("paused", Some(&unit('a')), &unit('b'));
    let mut records = scripted(vec![Some(bytes)], vec![]);

    assert!(matches!(
        select_release(&mut records),
        Err(ReleaseSelectionError::InvalidJournal(_))
    ));
}

#[test]
fn a_journal_cannot_name_a_path_for_its_prior() {
    let bytes = serde_json::json!({
        "disposition": "forward",
        "prior_active_unit": r"..\..\Windows\System32",
    })
    .to_string()
    .into_bytes();
    let mut records = scripted(vec![Some(bytes)], vec![]);

    assert!(matches!(
        select_release(&mut records),
        Err(ReleaseSelectionError::InvalidJournal(_))
    ));
}

#[test]
fn a_pointer_of_another_schema_or_shape_is_refused() {
    for bytes in [
        br#"{"schema_version":2,"unit":"aa"}"#.to_vec(),
        format!(r#"{{"schema_version":2,"unit":"{}"}}"#, "a".repeat(64)).into_bytes(),
        format!(
            r#"{{"schema_version":1,"unit":"{}","extra":1}}"#,
            "a".repeat(64)
        )
        .into_bytes(),
        br#"{"schema_version":1,"unit":"..\\..\\evil"}"#.to_vec(),
    ] {
        let mut records = scripted(vec![None, None], vec![Some(bytes)]);

        assert!(matches!(
            select_release(&mut records),
            Err(ReleaseSelectionError::InvalidPointer(_))
        ));
    }
}

#[test]
fn a_read_failure_is_reported() {
    struct Unreadable;
    impl ReleaseRecords for Unreadable {
        fn read_journal(&mut self) -> io::Result<Option<Vec<u8>>> {
            Err(io::ErrorKind::PermissionDenied.into())
        }
        fn read_pointer(&mut self) -> io::Result<Option<Vec<u8>>> {
            Ok(None)
        }
    }

    assert!(matches!(
        select_release(&mut Unreadable),
        Err(ReleaseSelectionError::Read(_))
    ));
}
