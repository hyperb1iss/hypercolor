//! How a stopped broker's last exit reads to a person.

use hypercolor_windows_pawnio::{
    BROKER_EXIT_RUNTIME_FAILED, BROKER_EXIT_SERVE_FAILED, describe_broker_exit,
};

#[test]
fn a_clean_stop_says_something_stopped_it() {
    let exit = describe_broker_exit(0, 0);

    assert!(exit.contains("exit code 0"), "{exit}");
    assert!(exit.contains("stopped on purpose"), "{exit}");
}

#[test]
fn a_service_that_never_started_since_boot_says_so() {
    assert!(describe_broker_exit(1077, 0).contains("not been started since Windows booted"));
}

#[test]
fn broker_exit_codes_name_the_step_that_failed() {
    let serve = describe_broker_exit(1066, BROKER_EXIT_SERVE_FAILED);
    let runtime = describe_broker_exit(1066, BROKER_EXIT_RUNTIME_FAILED);
    let unknown = describe_broker_exit(1066, 42);

    assert!(serve.contains("request loop failed"), "{serve}");
    assert!(runtime.contains("could not start its runtime"), "{runtime}");
    assert!(unknown.contains("broker exit code 42"), "{unknown}");
}

#[test]
fn a_crash_and_a_start_timeout_are_told_apart() {
    assert!(describe_broker_exit(1067, 0).contains("crash"));
    assert!(describe_broker_exit(1053, 0).contains("did not answer Windows in time"));
}

#[test]
fn any_other_code_is_reported_verbatim() {
    assert!(describe_broker_exit(2, 0).contains("Windows error 2"));
}
