//! How a keyed scene write's response reads: an answer that settles it,
//! one from another daemon run, or no answer, to be sent again under the
//! same key.

use hypercolor_ui::api::client::{ApiError, KeyedResponse, MutationOutcome};
use hypercolor_ui::components::layout_builder::keyed::{Answer, TRY_AGAIN_CODE, classify};

fn answered(
    run: Option<&str>,
    status: u16,
    outcome: Result<MutationOutcome<()>, ApiError>,
) -> KeyedResponse<()> {
    KeyedResponse {
        run: run.map(str::to_owned),
        status: Some(status),
        outcome,
    }
}

fn refused(status: u16, code: &str) -> Result<MutationOutcome<()>, ApiError> {
    Err(ApiError::Http {
        status,
        code: Some(code.to_owned()),
        message: None,
        details: None,
    })
}

#[test]
fn a_daemon_that_cannot_take_the_write_now_is_asked_again_under_the_key() {
    let busy = answered(Some("run-a"), 503, refused(503, TRY_AGAIN_CODE));
    assert!(matches!(classify("run-a", busy), Answer::Lost(_)));
    // Another 503 is the daemon's final word on the write.
    let gone = answered(Some("run-a"), 503, refused(503, "device_unavailable"));
    assert!(matches!(
        classify("run-a", gone),
        Answer::Final { status: 503, .. }
    ));
}

#[test]
fn a_write_given_up_on_at_its_deadline_is_no_answer() {
    let timed_out = KeyedResponse::<()> {
        run: None,
        status: None,
        outcome: Err(ApiError::Network("no answer within 10 s".to_owned())),
    };
    assert!(matches!(classify("run-a", timed_out), Answer::Lost(_)));
}

#[test]
fn only_the_write_s_own_run_settles_it() {
    let applied = || Ok(MutationOutcome::Applied(()));
    assert!(matches!(
        classify("run-a", answered(Some("run-a"), 200, applied())),
        Answer::Final { status: 200, .. }
    ));
    assert!(matches!(
        classify("run-a", answered(Some("run-b"), 200, applied())),
        Answer::OtherRun
    ));
    // Something that is not the daemon (a proxy's 502) names no run.
    assert!(matches!(
        classify("run-a", answered(None, 502, refused(502, "bad_gateway"))),
        Answer::Lost(_)
    ));
    // A daemon that names no runs is taken at its word.
    assert!(matches!(
        classify("", answered(None, 200, applied())),
        Answer::Final { status: 200, .. }
    ));
}
