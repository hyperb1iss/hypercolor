//! The Win32 host for the update pipe client.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt as _;

use hypercolor_platform_fs::windows_named_pipe_server_process_id;
use windows_service::service::ServiceAccess;
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use super::{UPDATE_SERVICE_NAME, UpdatePipeHost, UpdatePipeName};

/// `SECURITY_IDENTIFICATION`: the server may learn who the client is, but
/// never act as the client, even if it is not the update service.
const SECURITY_IDENTIFICATION: u32 = 0x0001_0000;
/// `ERROR_SERVICE_DOES_NOT_EXIST`.
const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;

/// Connects through the file system, names the server with
/// `GetNamedPipeServerProcessId`, and asks the service control manager for
/// the service's process with `QueryServiceStatusEx`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsUpdatePipeHost {
    service: String,
}

impl Default for WindowsUpdatePipeHost {
    fn default() -> Self {
        Self::for_service(UPDATE_SERVICE_NAME)
    }
}

impl WindowsUpdatePipeHost {
    /// The host for `HypercolorUpdate`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The host for a pipe `service` must serve, for harnesses that stand
    /// in a registered test service.
    #[must_use]
    pub fn for_service(service: &str) -> Self {
        Self {
            service: service.to_owned(),
        }
    }
}

impl UpdatePipeHost for WindowsUpdatePipeHost {
    type Pipe = File;

    fn connect(&self, name: &UpdatePipeName) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(name.as_str())
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
}

fn service_error(error: windows_service::Error) -> io::Error {
    match error {
        windows_service::Error::Winapi(error) => error,
        other => io::Error::other(other.to_string()),
    }
}
