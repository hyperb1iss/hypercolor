//! Windows-specific hardware identification via WMI.

use hypercolor_types::motherboard::MotherboardInfo;
use serde::Deserialize;
use tracing::debug;

#[allow(non_camel_case_types)]
#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct Win32_BaseBoard {
    manufacturer: Option<String>,
    product: Option<String>,
    version: Option<String>,
}

pub(super) fn motherboard_info() -> Option<MotherboardInfo> {
    let con = match wmi::WMIConnection::new() {
        Ok(con) => con,
        Err(err) => {
            debug!("WMI connection failed for motherboard query: {err}");
            return None;
        }
    };
    let results: Vec<Win32_BaseBoard> = match con.query() {
        Ok(rows) => rows,
        Err(err) => {
            debug!("Win32_BaseBoard query failed: {err}");
            return None;
        }
    };

    let row = results.into_iter().next()?;
    let manufacturer = row
        .manufacturer
        .map(trim_string)
        .filter(|s| !s.is_empty())?;
    let product = row.product.map(trim_string).filter(|s| !s.is_empty())?;
    let version = row.version.map(trim_string).filter(|s| !s.is_empty());

    Some(MotherboardInfo {
        manufacturer,
        product,
        version,
    })
}

fn trim_string(value: String) -> String {
    value.trim().to_owned()
}
