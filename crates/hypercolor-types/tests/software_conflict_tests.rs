//! Competing-software contracts: the device rule and the wire shape.

use hypercolor_types::api::system::{SoftwareConflict, SoftwareConflictsStatus};

fn conflict(driver_ids: &[&str], all_drivers: bool, smbus: bool) -> SoftwareConflict {
    SoftwareConflict {
        id: "tool".to_owned(),
        name: "Tool".to_owned(),
        matched: vec!["Tool.exe".to_owned()],
        driver_ids: driver_ids.iter().map(|id| (*id).to_owned()).collect(),
        all_drivers,
        smbus,
        remedy: "Quit Tool.".to_owned(),
    }
}

#[test]
fn a_family_tool_affects_only_its_drivers() {
    let lconnect = conflict(&["lianli"], false, false);
    assert!(lconnect.affects("lianli", false));
    assert!(!lconnect.affects("razer", false));
    assert!(
        !lconnect.affects("asus", true),
        "no SMBus claim, no SMBus hit"
    );
}

#[test]
fn a_suite_affects_every_driver() {
    let suite = conflict(&[], true, false);
    assert!(suite.affects("nollie", false));
    assert!(suite.affects("wled", false));
}

#[test]
fn an_smbus_tool_affects_every_smbus_device() {
    let armoury = conflict(&["asus"], false, true);
    assert!(armoury.affects("asus", false));
    assert!(
        armoury.affects("corsair", true),
        "RAM on the bus is contested whichever driver owns it"
    );
    assert!(!armoury.affects("corsair", false));
}

#[test]
fn status_round_trips_with_snake_case_fields() {
    let status = SoftwareConflictsStatus {
        supported: true,
        scanned: true,
        scan_failed: false,
        conflicts: vec![conflict(&["lianli"], false, false)],
    };
    let value = serde_json::to_value(&status).expect("serialize status");
    assert_eq!(value["supported"], true);
    assert_eq!(value["conflicts"][0]["driver_ids"][0], "lianli");
    assert_eq!(value["conflicts"][0]["all_drivers"], false);
    let decoded: SoftwareConflictsStatus =
        serde_json::from_value(value).expect("deserialize status");
    assert_eq!(decoded, status);
}

#[test]
fn the_default_status_claims_nothing() {
    let status = SoftwareConflictsStatus::default();
    assert!(!status.supported);
    assert!(!status.scanned);
    assert!(!status.scan_failed);
    assert!(status.conflicts.is_empty());
}

#[test]
fn an_older_client_payload_without_scan_failed_still_parses() {
    let status: SoftwareConflictsStatus = serde_json::from_value(serde_json::json!({
        "supported": true,
        "scanned": true,
        "conflicts": []
    }))
    .expect("scan_failed defaults");
    assert!(!status.scan_failed);
}
