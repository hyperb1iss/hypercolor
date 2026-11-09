use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{self, Cursor, Read, Write};
use std::rc::Rc;

use super::{
    UPDATE_PIPE_QUIT, UPDATE_SERVICE_NAME, UpdatePipeError, UpdatePipeHost, UpdatePipeName,
    UpdateServiceEnd, await_quit_request, connect_update_pipe, follow_update_service,
};

const NAME: &str = r"\\.\pipe\hypercolor-update-0123456789abcdef0123456789abcdef";

/// A pipe that records whether anything read from or wrote to it.
#[derive(Debug, Clone, Default)]
struct FakePipe {
    touched: Rc<RefCell<bool>>,
    sent: Rc<RefCell<Vec<u8>>>,
}

impl Read for FakePipe {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        *self.touched.borrow_mut() = true;
        let message = b"stop";
        buffer[..message.len()].copy_from_slice(message);
        Ok(message.len())
    }
}

impl Write for FakePipe {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        *self.touched.borrow_mut() = true;
        self.sent.borrow_mut().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A host whose answers each test chooses, recording the order it is asked.
struct FakeHost {
    pipe: FakePipe,
    server: io::Result<u32>,
    service: io::Result<Option<u32>>,
    connect_fails: bool,
    calls: RefCell<Vec<&'static str>>,
}

impl FakeHost {
    fn new(server: u32, service: Option<u32>) -> Self {
        Self {
            pipe: FakePipe::default(),
            server: Ok(server),
            service: Ok(service),
            connect_fails: false,
            calls: RefCell::new(Vec::new()),
        }
    }
}

fn copy<T: Copy>(result: &io::Result<T>) -> io::Result<T> {
    match result {
        Ok(value) => Ok(*value),
        Err(error) => Err(io::Error::new(error.kind(), error.to_string())),
    }
}

impl UpdatePipeHost for FakeHost {
    type Pipe = FakePipe;

    fn connect(&self, name: &UpdatePipeName) -> io::Result<FakePipe> {
        assert_eq!(name.as_str(), NAME);
        self.calls.borrow_mut().push("connect");
        if self.connect_fails {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no pipe"));
        }
        Ok(self.pipe.clone())
    }

    fn server_process_id(&self, _pipe: &FakePipe) -> io::Result<u32> {
        self.calls.borrow_mut().push("server");
        copy(&self.server)
    }

    fn service_process_id(&self) -> io::Result<Option<u32>> {
        self.calls.borrow_mut().push("service");
        copy(&self.service)
    }

    fn read_record(&self) -> io::Result<Vec<u8>> {
        unreachable!("verification reads no record")
    }

    fn wait_for_run(&self) -> io::Result<bool> {
        unreachable!("verification waits on nothing")
    }
}

fn name() -> UpdatePipeName {
    UpdatePipeName::parse(NAME).expect("update pipe name")
}

#[test]
fn random_bits_make_a_name_the_client_accepts() {
    let name = UpdatePipeName::from_random([0xA5; 16]);

    assert_eq!(
        name.as_str(),
        format!(r"\\.\pipe\hypercolor-update-{}", "a5".repeat(16))
    );
    UpdatePipeName::parse(name.as_str()).expect("the client accepts it");
}

#[test]
fn a_published_record_round_trips_its_name() {
    let name = name();

    let record = name.encode_record().expect("record");

    assert_eq!(UpdatePipeName::decode_record(&record).expect("name"), name);
}

#[test]
fn a_record_cannot_steer_the_client_at_another_pipe() {
    let foreign = serde_json::json!({
        "schema_version": 1,
        "name": r"\\server\pipe\hypercolor-update-0123456789abcdef0123456789abcdef",
    });
    assert!(matches!(
        UpdatePipeName::decode_record(foreign.to_string().as_bytes()),
        Err(UpdatePipeError::InvalidName(_))
    ));

    let other_schema = serde_json::json!({ "schema_version": 2, "name": NAME });
    assert!(matches!(
        UpdatePipeName::decode_record(other_schema.to_string().as_bytes()),
        Err(UpdatePipeError::InvalidRecord(_))
    ));
    assert!(matches!(
        UpdatePipeName::decode_record(b"{"),
        Err(UpdatePipeError::InvalidRecord(_))
    ));
}

#[test]
fn only_published_update_pipe_names_are_accepted() {
    assert_eq!(name().as_str(), NAME);
    for text in [
        "",
        r"\\.\pipe\hypercolor-update-",
        r"\\.\pipe\hypercolor-update-0123456789abcdef0123456789abcde",
        r"\\.\pipe\hypercolor-update-0123456789abcdef0123456789abcdef0",
        r"\\.\pipe\hypercolor-update-0123456789ABCDEF0123456789ABCDEF",
        r"\\.\pipe\hypercolor-update-0123456789abcdef0123456789abcdeg",
        r"\\.\pipe\hypercolor-smbus-v1",
        r"\\server\pipe\hypercolor-update-0123456789abcdef0123456789abcdef",
        r"C:\hypercolor-update-0123456789abcdef0123456789abcdef",
        r"\\.\pipe\hypercolor-update-0123456789abcdef0123456789abc\..",
    ] {
        assert!(
            matches!(
                UpdatePipeName::parse(text),
                Err(UpdatePipeError::InvalidName(_))
            ),
            "{text:?} must be refused"
        );
    }
}

#[test]
fn a_pipe_the_service_process_serves_is_handed_over() {
    let host = FakeHost::new(4242, Some(4242));
    let mut pipe = connect_update_pipe(&host, &name()).expect("verified");
    assert_eq!(pipe.server_process_id(), 4242);
    assert!(!*host.pipe.touched.borrow(), "verification reads nothing");
    assert_eq!(*host.calls.borrow(), ["connect", "server", "service"]);

    let mut message = [0_u8; 4];
    pipe.read_exact(&mut message).expect("read through");
    assert_eq!(&message, b"stop");
    pipe.write_all(b"ok").expect("write through");
    pipe.flush().expect("flush");
    assert_eq!(*host.pipe.sent.borrow(), b"ok");
    let _inner: FakePipe = pipe.into_inner();
}

#[test]
fn a_pipe_any_other_process_serves_is_refused_unread() {
    let host = FakeHost::new(31337, Some(4242));
    assert!(matches!(
        connect_update_pipe(&host, &name()),
        Err(UpdatePipeError::NotTheService {
            server: 31337,
            service: 4242
        })
    ));
    assert!(!*host.pipe.touched.borrow());
}

#[test]
fn a_service_that_is_not_running_vouches_for_nothing() {
    for service in [None, Some(0)] {
        let host = FakeHost::new(0, service);
        assert!(
            matches!(
                connect_update_pipe(&host, &name()),
                Err(UpdatePipeError::ServiceNotRunning)
            ),
            "{service:?}"
        );
        assert!(!*host.pipe.touched.borrow());
    }
}

#[test]
fn host_failures_refuse_the_pipe() {
    let mut host = FakeHost::new(1, Some(1));
    host.connect_fails = true;
    assert!(matches!(
        connect_update_pipe(&host, &name()),
        Err(UpdatePipeError::Connect(_))
    ));
    assert_eq!(*host.calls.borrow(), ["connect"]);

    let mut host = FakeHost::new(1, Some(1));
    host.server = Err(io::Error::other("not a pipe"));
    assert!(matches!(
        connect_update_pipe(&host, &name()),
        Err(UpdatePipeError::ServerIdentity(_))
    ));
    assert_eq!(
        *host.calls.borrow(),
        ["connect", "server"],
        "the service is never asked about a pipe whose server is unknown"
    );

    let mut host = FakeHost::new(1, Some(1));
    host.service = Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
    let error = connect_update_pipe(&host, &name()).expect_err("refused");
    assert!(matches!(error, UpdatePipeError::ServiceQuery(_)));
    assert!(error.to_string().contains(UPDATE_SERVICE_NAME));
    assert!(!*host.pipe.touched.borrow());
}

/// One run of the fake service, as the client meets it.
struct Run {
    record: Vec<u8>,
    /// Whether the pipe is gone by the time the client connects.
    gone: bool,
    server: u32,
    service: u32,
    sends: &'static [u8],
}

impl Run {
    fn sending(sends: &'static [u8]) -> Self {
        Self {
            record: name().encode_record().expect("record"),
            gone: false,
            server: 7,
            service: 7,
            sends,
        }
    }
}

/// A service whose waits and runs each test scripts, recording what the
/// client did in order. Each wait that answers `true` reports one run.
#[derive(Default)]
struct FakeService {
    waits: RefCell<VecDeque<io::Result<bool>>>,
    runs: RefCell<VecDeque<Run>>,
    current: RefCell<Option<Run>>,
    calls: RefCell<Vec<&'static str>>,
}

impl FakeService {
    fn new(waits: impl IntoIterator<Item = bool>, runs: impl IntoIterator<Item = Run>) -> Self {
        Self {
            waits: RefCell::new(waits.into_iter().map(Ok).collect()),
            runs: RefCell::new(runs.into_iter().collect()),
            ..Self::default()
        }
    }

    fn run<T>(&self, answer: impl FnOnce(&Run) -> T) -> T {
        answer(self.current.borrow().as_ref().expect("a run in progress"))
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.borrow().clone()
    }
}

impl UpdatePipeHost for FakeService {
    type Pipe = Cursor<Vec<u8>>;

    fn connect(&self, _name: &UpdatePipeName) -> io::Result<Self::Pipe> {
        self.calls.borrow_mut().push("connect");
        if self.run(|run| run.gone) {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no pipe"));
        }
        Ok(self.run(|run| Cursor::new(run.sends.to_vec())))
    }

    fn server_process_id(&self, _pipe: &Self::Pipe) -> io::Result<u32> {
        Ok(self.run(|run| run.server))
    }

    fn service_process_id(&self) -> io::Result<Option<u32>> {
        Ok(Some(self.run(|run| run.service)))
    }

    fn read_record(&self) -> io::Result<Vec<u8>> {
        self.calls.borrow_mut().push("record");
        let run = self.runs.borrow_mut().pop_front().expect("a scripted run");
        let record = run.record.clone();
        *self.current.borrow_mut() = Some(run);
        Ok(record)
    }

    fn wait_for_run(&self) -> io::Result<bool> {
        self.calls.borrow_mut().push("run");
        self.waits
            .borrow_mut()
            .pop_front()
            .expect("a scripted wait")
    }
}

fn follow(service: &FakeService) -> (io::Result<UpdateServiceEnd>, Vec<UpdatePipeError>) {
    let mut lost = Vec::new();
    let end = follow_update_service(service, |error| lost.push(error));
    (end, lost)
}

#[test]
fn only_a_whole_quit_request_asks_the_app_to_quit() {
    assert_eq!(UPDATE_PIPE_QUIT, b"quit\n");
    assert!(await_quit_request(&b"quit\n"[..]).expect("a request"));
    assert!(
        !await_quit_request(&b""[..]).expect("a run that ended"),
        "a run that ends unasked asks nothing"
    );
    for sent in [&b"qu"[..], b"QUIT\n", b"stop\n", b"quit"] {
        assert!(
            matches!(
                await_quit_request(sent),
                Err(UpdatePipeError::UnknownRequest(_))
            ),
            "{sent:?}"
        );
    }
}

#[test]
fn a_run_that_asks_ends_the_following_at_once() {
    let service = FakeService::new([true], [Run::sending(UPDATE_PIPE_QUIT)]);

    let (end, lost) = follow(&service);

    assert_eq!(end.expect("followed"), UpdateServiceEnd::Quit);
    assert!(lost.is_empty());
    assert_eq!(service.calls(), ["run", "record", "connect"]);
}

#[test]
fn a_run_that_ends_unasked_leaves_the_client_waiting_for_the_next() {
    let service = FakeService::new(
        [true, true],
        [Run::sending(b""), Run::sending(UPDATE_PIPE_QUIT)],
    );

    let (end, lost) = follow(&service);

    assert_eq!(end.expect("followed"), UpdateServiceEnd::Quit);
    assert!(lost.is_empty());
    assert_eq!(
        service.calls(),
        ["run", "record", "connect", "run", "record", "connect"]
    );
}

#[test]
fn a_run_that_ends_before_the_client_arrives_asked_nothing() {
    let ended = Run {
        gone: true,
        ..Run::sending(UPDATE_PIPE_QUIT)
    };
    let service = FakeService::new([true, true], [ended, Run::sending(UPDATE_PIPE_QUIT)]);

    let (end, lost) = follow(&service);

    assert_eq!(end.expect("followed"), UpdateServiceEnd::Quit);
    assert!(lost.is_empty(), "{lost:?}");
    assert_eq!(
        service.calls(),
        ["run", "record", "connect", "run", "record", "connect"]
    );
}

#[test]
fn every_run_that_fails_is_reported_and_left() {
    let impostor = Run {
        server: 31337,
        ..Run::sending(UPDATE_PIPE_QUIT)
    };
    let foreign = Run {
        record: br#"{"schema_version":1,"name":"\\\\.\\pipe\\hypercolor-smbus-v1"}"#.to_vec(),
        ..Run::sending(UPDATE_PIPE_QUIT)
    };
    let service = FakeService::new(
        [true, true, true, true],
        [
            impostor,
            foreign,
            Run::sending(b"stop\n"),
            Run::sending(UPDATE_PIPE_QUIT),
        ],
    );

    let (end, lost) = follow(&service);

    assert_eq!(end.expect("followed"), UpdateServiceEnd::Quit);
    assert!(
        matches!(
            lost.as_slice(),
            [
                UpdatePipeError::NotTheService {
                    server: 31337,
                    service: 7
                },
                UpdatePipeError::InvalidName(_),
                UpdatePipeError::UnknownRequest(_),
            ]
        ),
        "{lost:?}"
    );
    assert_eq!(
        service.calls(),
        [
            "run", "record", "connect", "run", "record", "run", "record", "connect", "run",
            "record", "connect"
        ],
        "a failed run is never answered again"
    );
}

#[test]
fn a_service_that_is_not_registered_is_never_followed() {
    let service = FakeService::new([false], []);

    let (end, lost) = follow(&service);

    assert_eq!(end.expect("followed"), UpdateServiceEnd::Unregistered);
    assert!(lost.is_empty());
    assert_eq!(service.calls(), ["run"]);
}

#[test]
fn a_service_deleted_between_runs_ends_the_following() {
    let service = FakeService::new([true, false], [Run::sending(b"")]);

    let (end, _) = follow(&service);

    assert_eq!(end.expect("followed"), UpdateServiceEnd::Unregistered);
    assert_eq!(service.calls(), ["run", "record", "connect", "run"]);
}

#[test]
fn a_service_manager_that_cannot_be_asked_ends_the_following() {
    let service = FakeService::default();
    service.waits.borrow_mut().push_back(Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "denied",
    )));

    let (end, _) = follow(&service);

    assert_eq!(
        end.expect_err("an error").kind(),
        io::ErrorKind::PermissionDenied
    );
}

#[cfg(windows)]
mod native {
    use std::io::{Read as _, Write as _};
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::windows::named_pipe::ServerOptions;

    use super::super::{
        UpdatePipeError, UpdatePipeHost, UpdatePipeName, WindowsUpdatePipeHost, connect_update_pipe,
    };
    use crate::windows::WindowsInstallLayout;

    /// Long enough for any wait the tests expect to end.
    const WAIT_DEADLINE: Duration = Duration::from_secs(30);

    fn layout() -> WindowsInstallLayout {
        WindowsInstallLayout::of_launcher(
            Path::new(r"C:\Program Files\Hypercolor\Hypercolor.exe"),
            Path::new(r"C:\ProgramData"),
        )
        .expect("a layout")
    }

    fn host() -> WindowsUpdatePipeHost {
        WindowsUpdatePipeHost::new(&layout())
    }

    /// The real Win32 connect and server lookup, with the service's process
    /// chosen by the test.
    struct ServiceRunsIn(u32);

    impl UpdatePipeHost for ServiceRunsIn {
        type Pipe = std::fs::File;

        fn connect(&self, name: &UpdatePipeName) -> std::io::Result<std::fs::File> {
            host().connect(name)
        }

        fn server_process_id(&self, pipe: &std::fs::File) -> std::io::Result<u32> {
            host().server_process_id(pipe)
        }

        fn service_process_id(&self) -> std::io::Result<Option<u32>> {
            Ok(Some(self.0))
        }

        fn read_record(&self) -> std::io::Result<Vec<u8>> {
            unreachable!("these tests name the pipe themselves")
        }

        fn wait_for_run(&self) -> std::io::Result<bool> {
            unreachable!("these tests serve the pipe themselves")
        }
    }

    /// Tests run in parallel and the clock ticks in 100ns steps, so a
    /// counter keeps two names from one process apart.
    static SEQUENCE: AtomicU32 = AtomicU32::new(0);

    fn fresh_name() -> UpdatePipeName {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let random = format!(
            "{:08x}{sequence:08x}{:016x}",
            std::process::id(),
            nanos as u64
        );
        UpdatePipeName::parse(&format!(r"\\.\pipe\hypercolor-update-{random}")).expect("pipe name")
    }

    /// Serve one connection on `name` from this process: say "stop", then
    /// return whatever the client sent before it closed.
    fn serve<T: Send + 'static>(
        name: &UpdatePipeName,
        client: impl FnOnce(UpdatePipeName) -> T + Send + 'static,
    ) -> (T, Vec<u8>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let name = name.clone();
        runtime.block_on(async move {
            let mut server = ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(name.as_str())
                .expect("fake update service pipe");
            let client = tokio::task::spawn_blocking(move || client(name));
            let mut received = Vec::new();
            // A client that refuses may close before the server finishes
            // connecting, which ends the connection without a byte either way.
            let connected = tokio::time::timeout(Duration::from_secs(30), server.connect()).await;
            if matches!(connected, Ok(Ok(()))) {
                let _ = server.write_all(b"stop").await;
                let _ = server.read_to_end(&mut received).await;
            }
            (client.await.expect("client thread"), received)
        })
    }

    #[test]
    fn a_pipe_served_by_the_service_process_is_verified_and_usable() {
        let name = fresh_name();
        let own = std::process::id();
        let (message, received) = serve(&name, move |name| {
            let mut pipe = connect_update_pipe(&ServiceRunsIn(own), &name).expect("verified");
            assert_eq!(pipe.server_process_id(), own);
            let mut message = [0_u8; 4];
            pipe.read_exact(&mut message).expect("read the request");
            pipe.write_all(b"quitting").expect("answer");
            message
        });
        assert_eq!(&message, b"stop");
        assert_eq!(received, b"quitting");
    }

    #[test]
    fn a_pipe_served_by_any_other_process_is_refused_before_a_byte_moves() {
        let name = fresh_name();
        let own = std::process::id();
        let (refusal, received) = serve(&name, move |name| {
            match connect_update_pipe(&ServiceRunsIn(own + 4), &name) {
                Err(UpdatePipeError::NotTheService { server, service }) => (server, service),
                other => panic!("expected a refusal, got {other:?}"),
            }
        });
        assert_eq!(refusal, (own, own + 4));
        assert!(received.is_empty(), "the client sent nothing");
    }

    #[test]
    fn the_service_manager_names_a_running_service_process_and_no_other() {
        let event_log = WindowsUpdatePipeHost::for_service("EventLog", &layout())
            .service_process_id()
            .expect("ask the service control manager")
            .expect("the event log service always runs");
        assert_ne!(event_log, std::process::id());

        let name = fresh_name();
        let (refusal, _) = serve(&name, |name| {
            let event_log = WindowsUpdatePipeHost::for_service("EventLog", &layout());
            match connect_update_pipe(&event_log, &name) {
                Err(UpdatePipeError::NotTheService { service, .. }) => service,
                other => panic!("expected a refusal, got {other:?}"),
            }
        });
        assert_eq!(
            refusal, event_log,
            "a pipe this test serves is not the service's"
        );

        let missing =
            WindowsUpdatePipeHost::for_service("HypercolorUpdateTestMissingService", &layout());
        assert_eq!(missing.service_process_id().expect("ask"), None);
    }

    #[test]
    fn nothing_served_at_the_name_is_a_connect_failure() {
        assert!(matches!(
            connect_update_pipe(&host(), &fresh_name()),
            Err(UpdatePipeError::Connect(_))
        ));
    }

    /// Run `wait` on a thread and give its answer, or fail once it has
    /// taken longer than any wait the test expects to end.
    fn within_deadline<T: Send + 'static>(wait: impl FnOnce() -> T + Send + 'static) -> T {
        let (done, answer) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(wait());
        });
        answer
            .recv_timeout(WAIT_DEADLINE)
            .expect("the wait ended in time")
    }

    /// The event log service runs from boot to shutdown, so it started
    /// once, before any test, and never starts again while one runs.
    #[test]
    fn a_running_service_is_reported_once_and_then_only_when_it_starts_again() {
        let (reports, reported) = mpsc::channel();
        std::thread::spawn(move || {
            let host = WindowsUpdatePipeHost::for_service("EventLog", &layout());
            let _ = reports.send(host.wait_for_run());
            let _ = reports.send(host.wait_for_run());
        });

        assert!(
            reported
                .recv_timeout(WAIT_DEADLINE)
                .expect("a running service is reported at once")
                .expect("ask the service control manager"),
            "the run is reported"
        );
        assert!(
            reported.recv_timeout(Duration::from_secs(2)).is_err(),
            "the same run is not reported twice"
        );
    }

    #[test]
    fn a_service_that_is_not_registered_is_never_waited_for() {
        let reported = within_deadline(|| {
            WindowsUpdatePipeHost::for_service("HypercolorUpdateTestMissingService", &layout())
                .wait_for_run()
        });

        assert!(!reported.expect("ask the service control manager"));
    }

    #[test]
    fn the_record_is_read_from_the_activator_directory_and_bounded() {
        let root = tempfile::tempdir().expect("a temporary root");
        let layout =
            WindowsInstallLayout::of_launcher(&root.path().join("Hypercolor.exe"), root.path())
                .expect("a layout");
        let record = layout.update_pipe_record();
        std::fs::create_dir_all(record.parent().expect("the activator directory"))
            .expect("create the activator directory");
        let host = WindowsUpdatePipeHost::new(&layout);

        assert_eq!(
            host.read_record().expect_err("nothing published").kind(),
            std::io::ErrorKind::NotFound
        );

        let name = fresh_name();
        std::fs::write(&record, name.encode_record().expect("encode")).expect("publish");
        assert_eq!(
            UpdatePipeName::decode_record(&host.read_record().expect("read")).expect("a name"),
            name
        );

        std::fs::write(&record, vec![b' '; 2048]).expect("publish too much");
        assert_eq!(
            host.read_record().expect_err("too large").kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    /// A client that finds the one instance busy waits for it to listen
    /// again, which is how an app the update starts reaches a run whose
    /// last client it already let go.
    #[test]
    fn a_busy_pipe_is_waited_for_until_its_server_listens_again() {
        let name = fresh_name();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async move {
            let server = ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(name.as_str())
                .expect("fake update service pipe");
            let first = host().connect(&name).expect("the first client connects");
            server.connect().await.expect("accept the first client");
            server.disconnect().expect("let the first client go");
            drop(first);

            let (done, connected) = mpsc::channel();
            let waiting = name.clone();
            std::thread::spawn(move || {
                let _ = done.send(host().connect(&waiting).map(|_| ()));
            });
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert!(
                connected.try_recv().is_err(),
                "the second client waits while the instance is busy"
            );

            tokio::time::timeout(WAIT_DEADLINE, server.connect())
                .await
                .expect("the second client arrives in time")
                .expect("accept the second client");
            connected
                .recv_timeout(WAIT_DEADLINE)
                .expect("the wait ended")
                .expect("the second client connected");
        });
    }
}
