//! Running processes and services via WMI.

use hypercolor_types::host_software::{HostProcess, HostSoftwareSnapshot};
use serde::Deserialize;
use tracing::debug;

#[allow(non_camel_case_types)]
#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct Win32_Process {
    name: Option<String>,
    command_line: Option<String>,
}

#[allow(non_camel_case_types)]
#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct Win32_Service {
    name: Option<String>,
}

/// List running processes from `Win32_Process` and running services from
/// `Win32_Service`.
///
/// `CommandLine` is null for processes the daemon's account may not
/// inspect, which leaves those processes matchable by name only. Returns
/// `None` when WMI is unreachable or the process query fails; a failed
/// service query keeps the processes and reports no services.
#[must_use]
pub fn running_software() -> Option<HostSoftwareSnapshot> {
    let con = match wmi::WMIConnection::new() {
        Ok(con) => con,
        Err(err) => {
            debug!("WMI connection failed for the software inventory: {err}");
            return None;
        }
    };
    let processes: Vec<Win32_Process> =
        match con.raw_query("SELECT Name, CommandLine FROM Win32_Process") {
            Ok(rows) => rows,
            Err(err) => {
                debug!("Win32_Process query failed: {err}");
                return None;
            }
        };
    let services: Vec<Win32_Service> =
        match con.raw_query("SELECT Name FROM Win32_Service WHERE State = 'Running'") {
            Ok(rows) => rows,
            Err(err) => {
                debug!("Win32_Service query failed: {err}");
                Vec::new()
            }
        };

    Some(HostSoftwareSnapshot {
        processes: processes
            .into_iter()
            .filter_map(|row| {
                let name = row.name.filter(|name| !name.trim().is_empty())?;
                Some(HostProcess {
                    name,
                    command_line: row.command_line.filter(|line| !line.trim().is_empty()),
                })
            })
            .collect(),
        services: services
            .into_iter()
            .filter_map(|row| row.name.filter(|name| !name.trim().is_empty()))
            .collect(),
    })
}
