use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::future::Future;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use hypercolor_ui::api::client::{install_http_transport, install_verified_daemon_connection};
use hypercolor_ui::api::http_transport::{
    HttpMethod, HttpRequest, HttpRequestBody, HttpResponse, HttpTransport, HttpTransportError,
    HttpTransportFuture,
};
use hypercolor_ui::api::{
    DeviceSummary, SoftwareConflict, SoftwareConflictsStatus, fetch_software_conflicts,
    scan_software_conflicts,
};
use hypercolor_ui::software_conflicts::{
    CheckFeedback, DISMISSED_STORAGE_KEY, SOFTWARE_CONFLICTS_EVENT, STALE_SCAN_NOTE,
    banner_headline, banner_scope, check_feedback, conflicts_for_device, device_can_be_held,
    device_hint, device_hints, encode_dismissed, parse_dismissed, prune_dismissed,
    scan_is_conclusive, smbus_conflict_warning, stale_note, visible_conflicts,
};
use hypercolor_ui::ws::messages::{DEVICE_LIFECYCLE_EVENTS, extract_device_event_hint};

// ── Fixtures ────────────────────────────────────────────────────────────────

fn conflict(id: &str, name: &str) -> SoftwareConflict {
    SoftwareConflict {
        id: id.to_owned(),
        name: name.to_owned(),
        matched: vec![format!("{name}.exe")],
        driver_ids: Vec::new(),
        all_drivers: false,
        smbus: false,
        remedy: format!("Quit {name}."),
    }
}

fn suite(id: &str, name: &str) -> SoftwareConflict {
    SoftwareConflict {
        all_drivers: true,
        smbus: true,
        ..conflict(id, name)
    }
}

fn vendor_tool(id: &str, name: &str, drivers: &[&str], smbus: bool) -> SoftwareConflict {
    SoftwareConflict {
        driver_ids: drivers.iter().map(|driver| (*driver).to_owned()).collect(),
        smbus,
        ..conflict(id, name)
    }
}

fn smbus_tool(id: &str, name: &str) -> SoftwareConflict {
    SoftwareConflict {
        smbus: true,
        ..conflict(id, name)
    }
}

fn status(conflicts: Vec<SoftwareConflict>) -> SoftwareConflictsStatus {
    SoftwareConflictsStatus {
        supported: true,
        scanned: true,
        scan_failed: false,
        conflicts,
    }
}

fn device(id: &str, driver_id: &str, transport: &str, status: &str) -> DeviceSummary {
    serde_json::from_value(serde_json::json!({
        "id": id, "layout_device_id": id, "name": id,
        "status": status, "brightness": 100, "total_leds": 8,
        "origin": {"driver_id": driver_id, "backend_id": driver_id, "transport": transport},
        "presentation": {"label": driver_id}
    }))
    .expect("minimal summary uses contract defaults")
}

fn ids(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn names(conflicts: &[&SoftwareConflict]) -> Vec<String> {
    conflicts
        .iter()
        .map(|conflict| conflict.name.clone())
        .collect()
}

// ── Dismissal storage ───────────────────────────────────────────────────────

#[test]
fn dismissals_round_trip_through_storage_text() {
    let dismissed = ids(&["signalrgb", "razer_synapse"]);
    let encoded = encode_dismissed(&dismissed).expect("non-empty set encodes");
    assert_eq!(parse_dismissed(Some(&encoded)), dismissed);
    assert_eq!(DISMISSED_STORAGE_KEY, "hc-software-conflicts-dismissed");
}

#[test]
fn no_dismissals_clear_the_storage_key() {
    assert_eq!(encode_dismissed(&BTreeSet::new()), None);
}

#[test]
fn unreadable_storage_means_no_dismissals() {
    assert!(parse_dismissed(None).is_empty());
    assert!(parse_dismissed(Some("")).is_empty());
    assert!(parse_dismissed(Some("not json")).is_empty());
    assert!(parse_dismissed(Some(r#"{"signalrgb":true}"#)).is_empty());
    assert!(parse_dismissed(Some("[1, 2]")).is_empty());
}

// ── Dismissal lifetime ──────────────────────────────────────────────────────

#[test]
fn a_dismissal_clears_once_its_program_stops() {
    let dismissed = ids(&["signalrgb", "razer_synapse"]);
    let current = status(vec![suite("signalrgb", "SignalRGB")]);

    assert_eq!(prune_dismissed(&dismissed, &current), ids(&["signalrgb"]));
    assert!(prune_dismissed(&dismissed, &status(Vec::new())).is_empty());
}

#[test]
fn an_inconclusive_scan_keeps_every_dismissal() {
    let dismissed = ids(&["signalrgb"]);
    let unsupported = SoftwareConflictsStatus {
        supported: false,
        scanned: true,
        scan_failed: false,
        conflicts: Vec::new(),
    };
    let not_scanned = SoftwareConflictsStatus {
        supported: true,
        scanned: false,
        scan_failed: false,
        conflicts: Vec::new(),
    };

    assert!(!scan_is_conclusive(&unsupported));
    assert!(!scan_is_conclusive(&not_scanned));
    assert!(scan_is_conclusive(&status(Vec::new())));
    assert_eq!(prune_dismissed(&dismissed, &unsupported), dismissed);
    assert_eq!(prune_dismissed(&dismissed, &not_scanned), dismissed);
}

#[test]
fn dismissed_conflicts_are_hidden_and_order_is_kept() {
    let running = vec![
        suite("signalrgb", "SignalRGB"),
        vendor_tool("lian_li_l_connect", "Lian Li L-Connect", &["lianli"], false),
        vendor_tool("razer_synapse", "Razer Synapse", &["razer"], false),
    ];

    let visible = visible_conflicts(&running, &ids(&["lian_li_l_connect"]));
    let visible: Vec<&str> = visible.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(visible, ["signalrgb", "razer_synapse"]);
    assert_eq!(visible_conflicts(&running, &BTreeSet::new()).len(), 3);
}

#[test]
fn a_check_names_what_is_still_running_and_not_dismissed() {
    let current = status(vec![
        suite("signalrgb", "SignalRGB"),
        vendor_tool("razer_synapse", "Razer Synapse", &["razer"], false),
    ]);

    assert_eq!(
        check_feedback(&current, &BTreeSet::new()).map(|feedback| feedback.message()),
        Some("Still running: SignalRGB, Razer Synapse".to_owned())
    );
    assert_eq!(
        check_feedback(&current, &ids(&["signalrgb"])),
        Some(CheckFeedback::StillRunning("Razer Synapse".to_owned()))
    );
    assert_eq!(
        check_feedback(&current, &ids(&["signalrgb", "razer_synapse"])),
        None
    );
    assert_eq!(check_feedback(&status(Vec::new()), &BTreeSet::new()), None);
}

// ── Failed scans ────────────────────────────────────────────────────────────

fn failed(conflicts: Vec<SoftwareConflict>) -> SoftwareConflictsStatus {
    SoftwareConflictsStatus {
        scan_failed: true,
        ..status(conflicts)
    }
}

#[test]
fn a_failed_scan_keeps_dismissals_even_when_its_list_is_empty() {
    let dismissed = ids(&["signalrgb", "razer_synapse"]);

    assert!(!scan_is_conclusive(&failed(Vec::new())));
    assert_eq!(prune_dismissed(&dismissed, &failed(Vec::new())), dismissed);
    assert_eq!(
        prune_dismissed(&dismissed, &failed(vec![suite("signalrgb", "SignalRGB")])),
        dismissed
    );
}

#[test]
fn a_failed_scan_keeps_its_last_list_on_screen_with_a_note() {
    let last_good = failed(vec![suite("signalrgb", "SignalRGB")]);

    assert_eq!(stale_note(&last_good), Some(STALE_SCAN_NOTE));
    assert_eq!(
        visible_conflicts(&last_good.conflicts, &BTreeSet::new()).len(),
        1
    );
    assert_eq!(
        stale_note(&status(vec![suite("signalrgb", "SignalRGB")])),
        None
    );
}

#[test]
fn a_failed_scan_with_nothing_to_show_stays_silent() {
    let empty = failed(Vec::new());

    assert_eq!(stale_note(&empty), None);
    assert!(visible_conflicts(&empty.conflicts, &BTreeSet::new()).is_empty());
    assert_eq!(smbus_conflict_warning(&empty), None);
}

#[test]
fn a_failed_check_says_so_instead_of_listing_names() {
    let feedback = check_feedback(
        &failed(vec![suite("signalrgb", "SignalRGB")]),
        &BTreeSet::new(),
    );

    assert_eq!(feedback, Some(CheckFeedback::Failed));
    assert_eq!(
        feedback.map(|feedback| feedback.message()).as_deref(),
        Some("Check failed. The list shows the last check that worked.")
    );
}

#[test]
fn status_without_scan_failed_decodes_as_a_clean_scan() {
    let decoded: SoftwareConflictsStatus = serde_json::from_value(serde_json::json!({
        "supported": true, "scanned": true, "conflicts": []
    }))
    .expect("older daemons omit scan_failed");

    assert!(!decoded.scan_failed);
    assert!(scan_is_conclusive(&decoded));
}

// ── Which devices a program competes for ────────────────────────────────────

#[test]
fn device_matching_follows_the_shared_affects_rule() {
    let running = vec![
        vendor_tool("razer_synapse", "Razer Synapse", &["razer"], false),
        smbus_tool("msi_center", "MSI Center"),
        vendor_tool("lian_li_l_connect", "Lian Li L-Connect", &["lianli"], false),
    ];

    let keyboard = device("kbd", "razer", "usb", "connected");
    let ram = device("dimm", "asus", "smbus", "active");
    let strip = device("wled", "wled", "network", "active");

    assert_eq!(
        names(&conflicts_for_device(&keyboard, &running)),
        ["Razer Synapse"]
    );
    assert_eq!(names(&conflicts_for_device(&ram, &running)), ["MSI Center"]);
    assert!(conflicts_for_device(&strip, &running).is_empty());

    for conflict in &running {
        for target in [&keyboard, &ram, &strip] {
            let smbus =
                target.origin.transport == hypercolor_types::device::DriverTransportKind::Smbus;
            assert_eq!(
                conflicts_for_device(target, std::slice::from_ref(conflict)).len(),
                usize::from(conflict.affects(&target.origin.driver_id, smbus)),
            );
        }
    }
}

#[test]
fn whole_system_suites_compete_for_every_reachable_device() {
    let running = vec![suite("signalrgb", "SignalRGB")];
    for target in [
        device("kbd", "razer", "usb", "connected"),
        device("hue", "hue", "network", "known"),
        device("route", "openrgb", "bridge", "reconnecting"),
    ] {
        assert_eq!(
            names(&conflicts_for_device(&target, &running)),
            ["SignalRGB"]
        );
    }
}

#[test]
fn disabled_and_virtual_devices_carry_no_hint() {
    let running = vec![suite("signalrgb", "SignalRGB")];
    let disabled = device("kbd", "razer", "usb", "disabled");
    let simulator = device("sim", "simulator", "virtual", "active");
    let unplugged = device("fan", "lianli", "usb", "known");

    assert!(!device_can_be_held(&disabled));
    assert!(!device_can_be_held(&simulator));
    assert!(device_can_be_held(&unplugged));
    assert!(conflicts_for_device(&disabled, &running).is_empty());
    assert!(conflicts_for_device(&simulator, &running).is_empty());
}

// ── Copy ────────────────────────────────────────────────────────────────────

#[test]
fn device_hints_read_as_sentences() {
    let signal = suite("signalrgb", "SignalRGB");
    let icue = vendor_tool("corsair_icue", "Corsair iCUE", &["corsair"], true);
    let cooler = vendor_tool("coolercontrol", "CoolerControl", &["corsair"], false);

    assert_eq!(device_hint(&[]), None);
    assert_eq!(
        device_hint(&[&signal]).as_deref(),
        Some("SignalRGB is running and may be holding this device")
    );
    assert_eq!(
        device_hint(&[&signal, &icue]).as_deref(),
        Some("SignalRGB and Corsair iCUE are running and may be holding this device")
    );
    assert_eq!(
        device_hint(&[&signal, &icue, &cooler]).as_deref(),
        Some(
            "SignalRGB, Corsair iCUE, and CoolerControl are running and may be holding this device"
        )
    );
}

#[test]
fn device_hints_map_only_affected_devices() {
    let running = vec![vendor_tool(
        "razer_synapse",
        "Razer Synapse",
        &["razer"],
        false,
    )];
    let devices = vec![
        device("kbd", "razer", "usb", "active"),
        device("mouse", "razer", "usb", "disabled"),
        device("strip", "wled", "network", "active"),
    ];

    let hints = device_hints(&devices, &running);
    assert_eq!(hints.len(), 1);
    assert_eq!(
        hints.get("kbd").map(String::as_str),
        Some("Razer Synapse is running and may be holding this device")
    );
    assert!(device_hints(&devices, &[]).is_empty());
}

#[test]
fn banner_copy_names_the_program_and_what_it_competes_for() {
    assert_eq!(
        banner_headline(&suite("signalrgb", "SignalRGB")),
        "SignalRGB is running."
    );
    assert_eq!(
        banner_scope(&suite("signalrgb", "SignalRGB")),
        "It competes with Hypercolor for your devices."
    );
    assert_eq!(
        banner_scope(&vendor_tool(
            "lian_li_l_connect",
            "Lian Li L-Connect",
            &["lianli"],
            false
        )),
        "It competes with Hypercolor for Lian Li devices."
    );
    assert_eq!(
        banner_scope(&vendor_tool(
            "coolercontrol",
            "CoolerControl",
            &["corsair", "lianli", "asus"],
            false
        )),
        "It competes with Hypercolor for Corsair, Lian Li, and ASUS devices."
    );
    assert_eq!(
        banner_scope(&vendor_tool(
            "corsair_icue",
            "Corsair iCUE",
            &["corsair"],
            true
        )),
        "It competes with Hypercolor for Corsair devices and for motherboard, RAM, and GPU lighting."
    );
    assert_eq!(
        banner_scope(&smbus_tool("msi_center", "MSI Center")),
        "It competes with Hypercolor for motherboard, RAM, and GPU lighting."
    );
    assert_eq!(
        banner_scope(&vendor_tool(
            "future_tool",
            "Future Tool",
            &["mystery_driver"],
            false
        )),
        "It competes with Hypercolor for Mystery Driver devices."
    );
}

#[test]
fn smbus_card_warns_only_about_smbus_programs() {
    let running = vec![
        vendor_tool("lian_li_l_connect", "Lian Li L-Connect", &["lianli"], false),
        suite("signalrgb", "SignalRGB"),
        smbus_tool("msi_center", "MSI Center"),
    ];

    assert_eq!(
        smbus_conflict_warning(&status(running.clone())).as_deref(),
        Some(
            "Other RGB software is running: SignalRGB, MSI Center. Quit it first to avoid SMBus conflicts."
        )
    );
    assert_eq!(smbus_conflict_warning(&status(running[..1].to_vec())), None);
    assert_eq!(smbus_conflict_warning(&status(Vec::new())), None);
}

#[test]
fn smbus_card_says_when_its_list_may_be_out_of_date() {
    assert_eq!(
        smbus_conflict_warning(&failed(vec![suite("signalrgb", "SignalRGB")])).as_deref(),
        Some(
            "Other RGB software is running: SignalRGB. Quit it first to avoid SMBus conflicts. \
             The latest check failed, so this may be out of date."
        )
    );
}

// ── Live updates ────────────────────────────────────────────────────────────

#[test]
fn conflict_changes_reach_the_device_hint_channel() {
    assert_eq!(SOFTWARE_CONFLICTS_EVENT, "software_conflicts_changed");
    assert!(DEVICE_LIFECYCLE_EVENTS.contains(&SOFTWARE_CONFLICTS_EVENT));

    let data = serde_json::json!({ "count": 2 });
    let hint = extract_device_event_hint(SOFTWARE_CONFLICTS_EVENT, Some(&data))
        .expect("conflict changes produce a device hint");
    assert_eq!(hint.event_type, SOFTWARE_CONFLICTS_EVENT);
    assert_eq!(hint.device_id, None);
}

// ── REST client ─────────────────────────────────────────────────────────────

struct FakeHttpTransport {
    requests: Rc<RefCell<Vec<HttpRequest>>>,
    responses: RefCell<VecDeque<HttpResponse>>,
}

impl HttpTransport for FakeHttpTransport {
    fn send(&self, request: HttpRequest) -> HttpTransportFuture<'_> {
        self.requests.borrow_mut().push(request);
        let response = self
            .responses
            .borrow_mut()
            .pop_front()
            .expect("fake response queue exhausted");
        Box::pin(async move { Ok::<_, HttpTransportError>(response) })
    }
}

fn ready<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("fake transport future unexpectedly yielded"),
    }
}

fn envelope(data: &SoftwareConflictsStatus) -> HttpResponse {
    let body = serde_json::json!({
        "data": data,
        "meta": {
            "api_version": "1.0",
            "request_id": "req_conflicts_test",
            "timestamp": "1970-01-01T00:00:00Z"
        }
    });
    HttpResponse {
        status: 200,
        headers: Vec::new(),
        body: serde_json::to_vec(&body).expect("test response serializes"),
    }
}

#[test]
fn client_reads_and_rescans_through_the_conflict_endpoints() {
    let before = status(Vec::new());
    let after = status(vec![suite("signalrgb", "SignalRGB")]);
    let requests = Rc::new(RefCell::new(Vec::new()));
    install_verified_daemon_connection("http://127.0.0.1:9420", None);
    install_http_transport(Rc::new(FakeHttpTransport {
        requests: Rc::clone(&requests),
        responses: RefCell::new(VecDeque::from([envelope(&before), envelope(&after)])),
    }))
    .expect("first install succeeds");

    assert_eq!(
        ready(fetch_software_conflicts()).expect("GET parses"),
        before
    );
    assert_eq!(
        ready(scan_software_conflicts()).expect("POST parses"),
        after
    );

    let requests = requests.borrow();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, HttpMethod::Get);
    assert_eq!(requests[0].path, "/api/v1/system/conflicts");
    assert_eq!(requests[1].method, HttpMethod::Post);
    assert_eq!(requests[1].path, "/api/v1/system/conflicts/scan");
    assert_eq!(requests[1].body, HttpRequestBody::Empty);
}
