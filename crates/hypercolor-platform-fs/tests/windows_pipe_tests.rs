#![cfg(windows)]

use std::fs::OpenOptions;
use std::os::windows::io::{FromRawHandle as _, OwnedHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use hypercolor_platform_fs::windows_named_pipe_server_process_id;
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
