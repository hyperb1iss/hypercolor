//! The Win32 host for the update pipe client.

use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::{self, Read as _};
use std::os::windows::fs::OpenOptionsExt as _;
use std::path::PathBuf;

use hypercolor_platform_fs::{windows_named_pipe_server_process_id, windows_wait_for_pipe};
use hypercolor_windows_install_native::ServiceStarts;
use windows_service::service::ServiceAccess;
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use super::{MAX_UPDATE_PIPE_RECORD_BYTES, UPDATE_SERVICE_NAME, UpdatePipeHost, UpdatePipeName};
use crate::windows::WindowsInstallLayout;

/// `SECURITY_IDENTIFICATION`: the server may learn who the client is, but
/// never act as the client, even if it is not the update service.
const SECURITY_IDENTIFICATION: u32 = 0x0001_0000;
/// `ERROR_SERVICE_DOES_NOT_EXIST`.
const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;
/// `ERROR_PIPE_BUSY`: every instance has a client, or listens for none.
const ERROR_PIPE_BUSY: i32 = 231;

/// Connects through the file system, names the server with
/// `GetNamedPipeServerProcessId`, asks the service control manager for the
/// service's process with `QueryServiceStatusEx` and waits for its runs
/// with `NotifyServiceStatusChangeW`. One host remembers which runs it
/// reported, so a follower keeps the same host for its whole life.
#[derive(Debug)]
pub struct WindowsUpdatePipeHost {
    service: String,
    record: PathBuf,
    starts: RefCell<ServiceStarts>,
}

impl WindowsUpdatePipeHost {
    /// The host for `HypercolorUpdate` of the installation at `layout`.
    #[must_use]
    pub fn new(layout: &WindowsInstallLayout) -> Self {
        Self::for_service(UPDATE_SERVICE_NAME, layout)
    }

    /// The host for a pipe `service` must serve, for harnesses that stand
    /// in a registered test service.
    #[must_use]
    pub fn for_service(service: &str, layout: &WindowsInstallLayout) -> Self {
        Self {
            service: service.to_owned(),
            record: layout.update_pipe_record(),
            starts: RefCell::new(ServiceStarts::new(service)),
        }
    }
}

impl UpdatePipeHost for WindowsUpdatePipeHost {
    type Pipe = File;

    fn connect(&self, name: &UpdatePipeName) -> io::Result<File> {
        loop {
            match OpenOptions::new()
                .read(true)
                .write(true)
                .security_qos_flags(SECURITY_IDENTIFICATION)
                .open(name.as_str())
            {
                // The instance listens again once its server is ready for
                // another client, and the wait ends with an error once the
                // pipe is gone.
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    windows_wait_for_pipe(name.as_str())?;
                }
                connected => return connected,
            }
        }
    }

    fn server_process_id(&self, pipe: &File) -> io::Result<u32> {
        windows_named_pipe_server_process_id(pipe)
    }

    fn service_process_id(&self) -> io::Result<Option<u32>> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(service_error)?;
        let service = match manager.open_service(&self.service, ServiceAccess::QUERY_STATUS) {
            Ok(service) => service,
            Err(windows_service::Error::Winapi(error))
                if error.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(service_error(error)),
        };
        let status = service.query_status().map_err(service_error)?;
        Ok(status.process_id.filter(|&process| process != 0))
    }

    fn read_record(&self) -> io::Result<Vec<u8>> {
        let mut record = Vec::new();
        File::open(&self.record)?
            .take(MAX_UPDATE_PIPE_RECORD_BYTES + 1)
            .read_to_end(&mut record)?;
        if record.len() as u64 > MAX_UPDATE_PIPE_RECORD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is too large to be a pipe record", self.record.display()),
            ));
        }
        Ok(record)
    }

    fn wait_for_run(&self) -> io::Result<bool> {
        self.starts.borrow_mut().wait()
    }
}

fn service_error(error: windows_service::Error) -> io::Error {
    match error {
        windows_service::Error::Winapi(error) => error,
        other => io::Error::other(other.to_string()),
    }
}
