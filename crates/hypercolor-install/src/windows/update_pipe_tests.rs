use std::cell::RefCell;
use std::io::{self, Read, Write};
use std::rc::Rc;

use super::{
    UPDATE_SERVICE_NAME, UpdatePipeError, UpdatePipeHost, UpdatePipeName, connect_update_pipe,
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
}

fn name() -> UpdatePipeName {
    UpdatePipeName::parse(NAME).expect("update pipe name")
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

#[cfg(windows)]
mod native {
    use std::io::{Read as _, Write as _};
    use std::time::{SystemTime, UNIX_EPOCH};

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::windows::named_pipe::ServerOptions;

    use super::super::{
        UpdatePipeError, UpdatePipeHost, UpdatePipeName, WindowsUpdatePipeHost, connect_update_pipe,
    };

    /// The real Win32 connect and server lookup, with the service's process
    /// chosen by the test.
    struct ServiceRunsIn(u32);

    impl UpdatePipeHost for ServiceRunsIn {
        type Pipe = std::fs::File;

        fn connect(&self, name: &UpdatePipeName) -> std::io::Result<std::fs::File> {
            WindowsUpdatePipeHost::new().connect(name)
        }

        fn server_process_id(&self, pipe: &std::fs::File) -> std::io::Result<u32> {
            WindowsUpdatePipeHost::new().server_process_id(pipe)
        }

        fn service_process_id(&self) -> std::io::Result<Option<u32>> {
            Ok(Some(self.0))
        }
    }

    fn fresh_name() -> UpdatePipeName {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let random = format!("{:016x}{:016x}", std::process::id(), nanos as u64);
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
            if server.connect().await.is_ok() {
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
        let event_log = WindowsUpdatePipeHost::for_service("EventLog")
            .service_process_id()
            .expect("ask the service control manager")
            .expect("the event log service always runs");
        assert_ne!(event_log, std::process::id());

        let name = fresh_name();
        let (refusal, _) = serve(&name, |name| {
            match connect_update_pipe(&WindowsUpdatePipeHost::for_service("EventLog"), &name) {
                Err(UpdatePipeError::NotTheService { service, .. }) => service,
                other => panic!("expected a refusal, got {other:?}"),
            }
        });
        assert_eq!(
            refusal, event_log,
            "a pipe this test serves is not the service's"
        );

        let missing = WindowsUpdatePipeHost::for_service("HypercolorUpdateTestMissingService");
        assert_eq!(missing.service_process_id().expect("ask"), None);
    }

    #[test]
    fn nothing_served_at_the_name_is_a_connect_failure() {
        assert!(matches!(
            connect_update_pipe(&WindowsUpdatePipeHost::new(), &fresh_name()),
            Err(UpdatePipeError::Connect(_))
        ));
    }
}
