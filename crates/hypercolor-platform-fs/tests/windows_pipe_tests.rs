#![cfg(windows)]

use std::fs::{File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hypercolor_platform_fs::{
    WindowsSid, windows_adopt_inherited_pipe, windows_connect_pipe_client,
    windows_create_pipe_server, windows_named_pipe_server_process_id, windows_wait_for_pipe,
};
use windows_sys::Win32::Foundation::{ERROR_PIPE_BUSY, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{
    CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};

/// Long enough for any wait the tests expect to end; a wait that outlives
/// it never would have.
const WAIT_DEADLINE: Duration = Duration::from_secs(30);
/// How long a wait that must not end yet is watched.
const STILL_WAITING: Duration = Duration::from_millis(300);

#[test]
fn the_server_process_of_a_pipe_is_the_process_that_created_it() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let name = format!(
        r"\\.\pipe\hypercolor-platform-fs-test-{}-{nanos}",
        std::process::id()
    );
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // SAFETY: The name is NUL-terminated and outlives the call, and no
    // security attributes are passed.
    let raw = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            512,
            512,
            0,
            std::ptr::null(),
        )
    };
    assert_ne!(raw, INVALID_HANDLE_VALUE, "create the pipe server");
    // SAFETY: CreateNamedPipeW returned a new handle that nothing else owns.
    let _server = unsafe { OwnedHandle::from_raw_handle(raw) };

    let client = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&name)
        .expect("connect to the pipe");
    assert_eq!(
        windows_named_pipe_server_process_id(&client).expect("server process"),
        std::process::id()
    );

    let file = tempfile::tempfile().expect("regular file");
    assert!(
        windows_named_pipe_server_process_id(&file).is_err(),
        "a regular file has no pipe server"
    );
}

fn unique_pipe_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!(
        r"\\.\pipe\hypercolor-platform-fs-test-{}-{nanos}",
        std::process::id()
    )
}

fn own_pipe_sddl() -> String {
    let user = WindowsSid::current_user().expect("current user");
    format!("D:P(A;;GRGW;;;{})", user.to_string_form())
}

#[test]
fn a_created_pipe_carries_bytes_to_a_client_of_its_creator() {
    let name = unique_pipe_name();
    let mut server = windows_create_pipe_server(&name, &own_pipe_sddl()).expect("create");

    let mut client = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&name)
        .expect("connect to the pipe");
    windows_connect_pipe_client(&server).expect("accept a client that already arrived");
    assert_eq!(
        windows_named_pipe_server_process_id(&client).expect("server process"),
        std::process::id()
    );

    server.write_all(b"quit").expect("write");
    let mut received = [0_u8; 4];
    client.read_exact(&mut received).expect("read");
    assert_eq!(&received, b"quit");
}

#[test]
fn a_pipe_name_someone_serves_cannot_be_created_again() {
    let name = unique_pipe_name();
    let _first = windows_create_pipe_server(&name, &own_pipe_sddl()).expect("create");

    windows_create_pipe_server(&name, &own_pipe_sddl())
        .expect_err("the first instance owns the name");
}

#[test]
fn a_pipe_dacl_that_omits_the_caller_refuses_it() {
    let name = unique_pipe_name();
    let _server = windows_create_pipe_server(&name, "D:P(A;;GA;;;SY)").expect("create");

    let error = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&name)
        .expect_err("only SYSTEM may connect");
    assert_eq!(
        error.kind(),
        std::io::ErrorKind::PermissionDenied,
        "{error}"
    );
}

#[test]
fn only_a_pipe_can_be_adopted() {
    let file = tempfile::tempfile().expect("a file");

    // SAFETY: A file is refused before anything takes ownership of it.
    let refused = unsafe { windows_adopt_inherited_pipe(file.as_raw_handle() as usize) };

    assert!(refused.is_err());
    drop(file);
}

fn open_client(name: &str) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open(name)
}

/// A server whose one instance a client reached and then lost: the
/// instance is disconnected and listens for nobody until the server
/// connects again.
fn disconnected_server(name: &str) -> File {
    let server = windows_create_pipe_server(name, &own_pipe_sddl()).expect("create");
    let first = open_client(name).expect("the first client connects");
    windows_connect_pipe_client(&server).expect("accept the first client");
    // SAFETY: The handle is live for the call.
    let disconnected = unsafe { DisconnectNamedPipe(server.as_raw_handle()) };
    assert_ne!(disconnected, 0, "disconnect the first client");
    drop(first);
    server
}

fn wait_in_background(name: &str) -> Receiver<io::Result<()>> {
    let (done, result) = mpsc::channel();
    let name = name.to_owned();
    std::thread::spawn(move || {
        let _ = done.send(windows_wait_for_pipe(&name));
    });
    result
}

#[test]
fn a_disconnected_instance_is_busy_until_it_listens_again() {
    let name = unique_pipe_name();
    let server = disconnected_server(&name);

    let busy = open_client(&name).expect_err("nobody listens");
    assert_eq!(busy.raw_os_error(), Some(ERROR_PIPE_BUSY as i32), "{busy}");

    let waited = wait_in_background(&name);
    assert!(
        waited.recv_timeout(STILL_WAITING).is_err(),
        "the wait lasts while the instance is busy"
    );
    let listening = server.try_clone().expect("a second server handle");
    let accepted = std::thread::spawn(move || windows_connect_pipe_client(&listening));
    waited
        .recv_timeout(WAIT_DEADLINE)
        .expect("the wait ends once the instance listens")
        .expect("an instance listens");

    let _client = open_client(&name).expect("the instance takes a client again");
    accepted
        .join()
        .expect("the server thread")
        .expect("the server accepts the client");
}

#[test]
fn a_wait_for_a_busy_pipe_ends_when_the_pipe_closes() {
    let name = unique_pipe_name();
    let server = disconnected_server(&name);

    let waited = wait_in_background(&name);
    assert!(
        waited.recv_timeout(STILL_WAITING).is_err(),
        "the wait lasts while the instance is busy"
    );
    drop(server);
    let error = waited
        .recv_timeout(WAIT_DEADLINE)
        .expect("the wait ends once the pipe is gone")
        .expect_err("nothing serves the name any more");
    assert_eq!(error.kind(), io::ErrorKind::NotFound, "{error}");
}

#[test]
fn a_wait_for_a_pipe_nobody_serves_ends_at_once() {
    let error = windows_wait_for_pipe(&unique_pipe_name()).expect_err("nothing serves it");

    assert_eq!(error.kind(), io::ErrorKind::NotFound, "{error}");
}
