#![cfg(windows)]

use std::fs::OpenOptions;
use std::io::{Read as _, Write as _};
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use hypercolor_platform_fs::{
    WindowsSid, windows_adopt_inherited_pipe, windows_connect_pipe_client,
    windows_create_pipe_server, windows_named_pipe_server_process_id,
};
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{
    CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};

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
