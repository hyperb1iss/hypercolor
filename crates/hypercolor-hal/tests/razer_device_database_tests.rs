//! Keeps the Razer accessory rows of the device database in step with the
//! HAL registry: a row marked supported or in progress must resolve to a
//! descriptor, and a researched row must not.

use hypercolor_hal::database::ProtocolDatabase;
use hypercolor_hal::drivers::razer::RAZER_VENDOR_ID;

const RAZER_TOML: &str = include_str!("../../../data/drivers/vendors/razer.toml");

/// Device types covered here. Keyboards and mice are audited separately.
const ACCESSORY_TYPES: [&str; 12] = [
    "mousepad",
    "headset",
    "microphone",
    "speakers",
    "aio",
    "fan_controller",
    "argb_controller",
    "monitor",
    "case",
    "desk",
    "strip",
    "other",
];

struct Row {
    pid: u16,
    name: String,
    device_type: String,
    status: String,
}

fn quoted(value: &str) -> String {
    value.trim().trim_matches('"').to_owned()
}

/// Minimal reader for the `[[devices]]` rows: `pid`, `name`, `type`, and
/// `status` are always single-line scalars in the vendor files.
fn rows() -> Vec<Row> {
    RAZER_TOML
        .split("[[devices]]")
        .skip(1)
        .map(|block| {
            let mut pid = None;
            let mut name = String::new();
            let mut device_type = String::new();
            let mut status = String::new();
            for line in block.lines() {
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                match key.trim() {
                    "pid" => {
                        let hex = value.trim().trim_start_matches("0x");
                        pid = Some(u16::from_str_radix(hex, 16).expect("pid is hex"));
                    }
                    "name" => name = quoted(value),
                    "type" => device_type = quoted(value),
                    "status" => status = quoted(value),
                    _ => {}
                }
            }
            Row {
                pid: pid.expect("every Razer row carries a pid"),
                name,
                device_type,
                status,
            }
        })
        .collect()
}

fn accessory_rows() -> Vec<Row> {
    rows()
        .into_iter()
        .filter(|row| ACCESSORY_TYPES.contains(&row.device_type.as_str()))
        .collect()
}

#[test]
fn accessory_rows_agree_with_the_hal_registry() {
    let rows = accessory_rows();
    assert!(rows.len() > 40, "expected the full accessory catalog");

    for row in rows {
        let registered = ProtocolDatabase::lookup(RAZER_VENDOR_ID, row.pid).is_some();
        match row.status.as_str() {
            "supported" | "in_progress" => assert!(
                registered,
                "{} (1532:{:04X}) is marked {} but no descriptor claims it",
                row.name, row.pid, row.status
            ),
            _ => assert!(
                !registered,
                "{} (1532:{:04X}) is marked {} but the HAL registers it",
                row.name, row.pid, row.status
            ),
        }
    }
}

#[test]
fn accessory_product_ids_are_unique() {
    let mut pids = accessory_rows()
        .into_iter()
        .map(|row| row.pid)
        .collect::<Vec<_>>();
    let total = pids.len();
    pids.sort_unstable();
    pids.dedup();
    assert_eq!(pids.len(), total, "a product ID appears on two rows");
}

#[test]
fn accessories_carry_their_reference_product_ids() {
    let rows = rows();
    let named = |pid: u16| {
        rows.iter()
            .find(|row| row.pid == pid)
            .map(|row| row.name.as_str())
    };

    assert_eq!(named(0x007E), Some("Mouse Dock Chroma"));
    assert_eq!(named(0x00A4), Some("Mouse Dock Pro"));
    assert_eq!(named(0x0F1D), Some("Mouse Bungee V3 Chroma"));
    assert_eq!(named(0x0F17), Some("Tomahawk ATX"));
    assert_eq!(named(0x0F19), Some("Kraken Kitty Edition"));
    assert_eq!(named(0x0F35), Some("Hanbo Chroma"));

    for unlisted in [0x0F05, 0x0F22, 0x0F23, 0x0F24, 0x0F44, 0x020F] {
        assert_eq!(
            named(unlisted),
            None,
            "1532:{unlisted:04X} belongs to no Razer accessory in either reference"
        );
    }
}

#[test]
fn held_accessories_stay_unregistered() {
    // Each needs a report format Hypercolor does not encode, or has no
    // public protocol facts at all.
    for pid in [
        0x0F1F, // Chroma Addressable RGB Controller: 320-byte ARGB report
        0x0F35, // Hanbo Chroma: own report layout
        0x0F2C, // Chroma Light Strip Set: undecoded
        0x0F3C, // PWM PC Fan Controller
        0x0F43, // Laptop Cooling Pad
        0x0F28, // Raptor 27 second product ID
        0x0548, // Leviathan V2 Pro
        0x055A, // Nommo V2 Pro
        0x055C, // Nommo V2
        0x0574, // Barracuda X Chroma
    ] {
        assert!(
            ProtocolDatabase::lookup(RAZER_VENDOR_ID, pid).is_none(),
            "1532:{pid:04X} should stay researched"
        );
    }
}
