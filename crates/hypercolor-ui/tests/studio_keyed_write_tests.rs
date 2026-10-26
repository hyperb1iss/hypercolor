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
        replayed: false,
        outcome,
    }
}

fn stale() -> Result<MutationOutcome<()>, ApiError> {
    Ok(MutationOutcome::Stale { current: 9 })
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
    assert!(matches!(classify("run-a", 0, busy), Answer::Lost(_)));
    // Another 503 is the daemon's final word on the write.
    let gone = answered(Some("run-a"), 503, refused(503, "device_unavailable"));
    assert!(matches!(
        classify("run-a", 0, gone),
        Answer::Final { status: 503, .. }
    ));
}

#[test]
fn a_write_given_up_on_at_its_deadline_is_no_answer() {
    let timed_out = KeyedResponse::<()> {
        run: None,
        status: None,
        replayed: false,
        outcome: Err(ApiError::Network("no answer within 10 s".to_owned())),
    };
    assert!(matches!(classify("run-a", 0, timed_out), Answer::Lost(_)));
}

#[test]
fn only_the_write_s_own_run_settles_it() {
    let applied = || Ok(MutationOutcome::Applied(()));
    assert!(matches!(
        classify("run-a", 0, answered(Some("run-a"), 200, applied())),
        Answer::Final { status: 200, .. }
    ));
    assert!(matches!(
        classify("run-a", 0, answered(Some("run-b"), 200, applied())),
        Answer::OtherRun
    ));
    // Something that is not the daemon (a proxy's 502) names no run.
    assert!(matches!(
        classify("run-a", 0, answered(None, 502, refused(502, "bad_gateway"))),
        Answer::Lost(_)
    ));
    // A daemon that names no runs is taken at its word.
    assert!(matches!(
        classify("", 0, answered(None, 200, applied())),
        Answer::Final { status: 200, .. }
    ));
}

#[test]
fn a_refusal_for_the_scene_tells_only_when_it_can_be_the_first_request_s() {
    let replay = |response: KeyedResponse<()>| KeyedResponse {
        replayed: true,
        ..response
    };
    // The first attempt's own refusal is final.
    assert!(matches!(
        classify("run-a", 0, answered(Some("run-a"), 412, stale())),
        Answer::Final { status: 412, .. }
    ));
    // So is a replay to the first retry: only the first request can have
    // stored it.
    assert!(matches!(
        classify("run-a", 1, replay(answered(Some("run-a"), 412, stale()))),
        Answer::Final { status: 412, .. }
    ));
    // A later replay may be serving a retry the daemon ran afresh after
    // letting the first answer go.
    assert!(matches!(
        classify("run-a", 2, replay(answered(Some("run-a"), 412, stale()))),
        Answer::Forgotten
    ));
    // Run afresh, a retry refused for what the scene holds may be meeting
    // what the first request did.
    for (status, outcome) in [
        (412, stale()),
        (409, refused(409, "conflict")),
        (404, refused(404, "not_found")),
    ] {
        assert!(matches!(
            classify("run-a", 1, answered(Some("run-a"), status, outcome)),
            Answer::Forgotten
        ));
    }
    // An apply always tells: had the first request landed, the fence
    // would have refused every later one.
    for replayed in [false, true] {
        let applied = KeyedResponse {
            replayed,
            ..answered(Some("run-a"), 200, Ok(MutationOutcome::Applied(())))
        };
        assert!(matches!(
            classify("run-a", 3, applied),
            Answer::Final { status: 200, .. }
        ));
    }
    // A refusal that does not turn on the scene says the same of both.
    assert!(matches!(
        classify(
            "run-a",
            1,
            answered(Some("run-a"), 422, refused(422, "validation_error"))
        ),
        Answer::Final { status: 422, .. }
    ));
    // The daemon still asks for the write again.
    assert!(matches!(
        classify(
            "run-a",
            1,
            answered(Some("run-a"), 503, refused(503, TRY_AGAIN_CODE))
        ),
        Answer::Lost(_)
    ));
}
