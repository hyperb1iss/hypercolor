//! The broker's pipe lifecycle, run for real on a pipe name of its own.
//!
//! Every request is a `Ping`, which the broker answers without loading
//! PawnIO or touching a bus.

use std::ops::Deref;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::{Duration, Instant};

use tokio::runtime::Runtime;
use tokio::sync::watch;
use windows_sys::Win32::System::Pipes::WaitNamedPipeW;

use super::{
    BrokerEnvelope, BrokerRequest, BrokerResponse, StopReason, accept_client, build_runtime,
    create_server_pipe, send_request_bytes, serve_until_shutdown, wait_for_client, wide_null,
};
use crate::PawnIoResult;

/// `ERROR_FILE_NOT_FOUND`: no instance of the pipe exists.
const ERROR_FILE_NOT_FOUND: i32 = 2;
/// `ERROR_OPERATION_ABORTED`: the I/O was cancelled.
const ERROR_OPERATION_ABORTED: i32 = 995;
const RELEASE_DEADLINE: Duration = Duration::from_secs(5);
/// Bounds each test's async work, so a broken wait fails instead of hanging.
const TEST_DEADLINE: Duration = Duration::from_secs(60);

/// The broker's runtime, shut down with a deadline when the test ends.
///
/// Dropping a runtime waits without limit for its blocking threads, so a
/// client wait that ignored its cancel would hang the test run instead of
/// failing the test.
struct TestRuntime(Option<Runtime>);

impl TestRuntime {
    fn new() -> Self {
        Self(Some(build_runtime().expect("broker runtime should build")))
    }
}

impl Deref for TestRuntime {
    type Target = Runtime;

    fn deref(&self) -> &Runtime {
        self.0.as_ref().expect("runtime is present until drop")
    }
}

impl Drop for TestRuntime {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_timeout(RELEASE_DEADLINE);
        }
    }
}

/// A pipe name no installed broker and no other test uses.
fn test_pipe_name(test: &str) -> String {
    format!(
        r"\\.\pipe\hypercolor-smbus-test-{}-{test}",
        std::process::id()
    )
}

/// True while any instance of `name` exists, listening or connected.
fn pipe_exists(name: &str) -> bool {
    let wide = wide_null(name);
    let available = unsafe {
        // SAFETY: `wide` is NUL-terminated UTF-16 that outlives the call.
        WaitNamedPipeW(wide.as_ptr(), 1)
    };
    available != 0 || std::io::Error::last_os_error().raw_os_error() != Some(ERROR_FILE_NOT_FOUND)
}

/// Whether every instance of `name` closes within [`RELEASE_DEADLINE`].
fn pipe_released(name: &str) -> bool {
    let deadline = Instant::now() + RELEASE_DEADLINE;
    while Instant::now() < deadline {
        if !pipe_exists(name) {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    !pipe_exists(name)
}

fn ping(name: &str) -> PawnIoResult<String> {
    let request = serde_json::to_vec(&BrokerRequest::Ping).expect("ping should encode");
    let response = send_request_bytes(name, &request)?;
    let envelope =
        serde_json::from_slice::<BrokerEnvelope>(&response).expect("envelope should decode");
    match envelope.into_result("ping")? {
        BrokerResponse::Pong { version } => Ok(version),
        other => panic!("ping answered with {other:?}"),
    }
}

#[test]
fn every_request_is_answered_and_shutdown_releases_the_pipe() {
    let name = test_pipe_name("serve");
    let runtime = TestRuntime::new();
    let (stop, stopped) = watch::channel(None);
    let client = {
        let name = name.clone();
        thread::spawn(move || {
            let answers = (0..200).map(|_| ping(&name)).collect::<Vec<_>>();
            let _ = stop.send(Some(StopReason::ConsoleInterrupt));
            answers
        })
    };

    let served = runtime.block_on(async {
        tokio::time::timeout(TEST_DEADLINE, serve_until_shutdown(&name, stopped)).await
    });
    served
        .expect("the serve loop should stop once shutdown is sent")
        .expect("the serve loop should not fail");
    for answer in client.join().expect("client thread should not panic") {
        assert_eq!(
            answer.expect("every request should be answered"),
            env!("CARGO_PKG_VERSION")
        );
    }
    // The loop always has a client wait pending when it stops. Stopping
    // must cancel that wait, or its blocking thread keeps the last
    // instance listening.
    assert!(
        pipe_released(&name),
        "a pipe instance outlived the serve loop"
    );
}

#[test]
fn a_dropped_client_wait_releases_its_pipe() {
    let name = test_pipe_name("dropped-wait");
    let runtime = TestRuntime::new();

    // Dropping the wait's future must cancel the connect its blocking
    // thread is waiting on.
    for attempt in 0..50 {
        runtime.block_on(async {
            let pipe = create_server_pipe(&name).expect("pipe instance should be created");
            let accepted = tokio::time::timeout(Duration::ZERO, accept_client(pipe)).await;
            assert!(accepted.is_err(), "no client opened the pipe");
        });
        assert!(
            pipe_released(&name),
            "attempt {attempt}: a dropped client wait kept its pipe instance"
        );
    }
}

#[test]
fn a_wait_cancelled_before_its_connect_is_issued_still_ends() {
    let name = test_pipe_name("cancelled-first");
    let pipe = create_server_pipe(&name).expect("pipe instance should be created");

    // The cancel already ran, finding no connect to cancel; the wait must
    // see the request and cancel the connect it issues itself.
    let error = wait_for_client(&pipe, &AtomicBool::new(true))
        .expect_err("a cancelled wait should not report a client");

    assert_eq!(error.raw_os_error(), Some(ERROR_OPERATION_ABORTED));
}

#[test]
fn an_accepted_pipe_reads_what_the_client_sent_first() {
    let name = test_pipe_name("first-read");
    let runtime = TestRuntime::new();
    let pipe = create_server_pipe(&name).expect("pipe instance should be created");
    let client = {
        let name = name.clone();
        thread::spawn(move || {
            // Give the server time to start waiting, so the client arrives
            // at an instance that has been listening on its own.
            thread::sleep(Duration::from_millis(100));
            ping(&name)
        })
    };

    let exchange = runtime.block_on(async {
        tokio::time::timeout(TEST_DEADLINE, async {
            let mut server = accept_client(pipe)
                .await
                .expect("the client should be accepted");
            let request = super::read_frame(&mut server)
                .await
                .expect("the first read should return the client's request");
            assert!(matches!(
                serde_json::from_slice::<BrokerRequest>(&request),
                Ok(BrokerRequest::Ping)
            ));
            let pong = BrokerEnvelope::ok(Ok(BrokerResponse::Pong {
                version: "test".to_owned(),
            }));
            let response = serde_json::to_vec(&pong).expect("pong should encode");
            super::write_frame(&mut server, &response)
                .await
                .expect("the response should be written");
        })
        .await
    });
    exchange.expect("the client should arrive and be served");

    assert_eq!(
        client
            .join()
            .expect("client thread should not panic")
            .expect("the client should get its answer"),
        "test"
    );
}
