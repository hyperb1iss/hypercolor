//! Contract tests for Studio drag and drop: the drop table, the spring
//! loaded tab, new zone naming, and announcements.

use hypercolor_types::spatial::NormalizedPosition;

use hypercolor_ui::pages::studio::drag_model::{
    CANCELLED_ANNOUNCEMENT, DragSource, DropAction, DropTarget, MovePayload, Origin, SpringLoad,
    SpringPhase, action_hint, channel_label, moved_announcement, new_zone_announcement,
    opened_announcement, origin_name, placed_announcement, resolve_drop, springs,
    zone_name_for_device,
};

const MAIN: &str = "main";
const KEYS: &str = "keyboard";

fn point(x: f32, y: f32) -> NormalizedPosition {
    NormalizedPosition::new(x, y)
}

fn canvas(zone: &str) -> DropTarget {
    DropTarget::Canvas {
        zone_id: zone.to_owned(),
        point: point(0.25, 0.75),
    }
}

fn placed_device() -> MovePayload {
    MovePayload::zone_device(
        "razer:huntsman",
        "Razer Huntsman V2",
        MAIN,
        vec!["out-1".into(), "out-2".into(), "out-3".into()],
    )
}

fn unassigned_device() -> MovePayload {
    MovePayload::unassigned_device("wled:theater", "WLED Theater", 2)
}

fn placed_channel() -> MovePayload {
    MovePayload::channel(
        "nollie:32",
        "Nollie 32",
        "Channel 4",
        Origin::Zone(MAIN.to_owned()),
        Some("ch-4".into()),
        Some("Channel 4".into()),
    )
}

fn unassigned_channel() -> MovePayload {
    MovePayload::channel(
        "nollie:32",
        "Nollie 32",
        "Channel 6",
        Origin::Unassigned,
        None,
        Some("Channel 6".into()),
    )
}

fn canvas_boxes() -> MovePayload {
    MovePayload::canvas(
        "razer:huntsman",
        "Razer Huntsman V2",
        "Razer Huntsman V2",
        MAIN,
        vec!["out-1".into(), "out-2".into()],
    )
}

fn pane_payloads() -> Vec<MovePayload> {
    vec![
        placed_device(),
        unassigned_device(),
        placed_channel(),
        unassigned_channel(),
    ]
}

// ── The drop table, one row at a time ─────────────────────────────────────

#[test]
fn pane_drags_onto_the_canvas_claim_for_the_open_zone_at_the_drop_point() {
    for payload in pane_payloads() {
        assert_eq!(
            resolve_drop(&payload, &canvas(MAIN)),
            Some(DropAction::Place {
                zone_id: MAIN.to_owned(),
                point: point(0.25, 0.75),
            }),
            "{payload:?}"
        );
    }
}

#[test]
fn a_device_from_another_zone_dropped_on_the_canvas_moves_in_at_the_point() {
    let from_keys = MovePayload::zone_device("razer:huntsman", "Razer", KEYS, vec!["k".into()]);
    assert_eq!(
        resolve_drop(&from_keys, &canvas(MAIN)),
        Some(DropAction::Place {
            zone_id: MAIN.to_owned(),
            point: point(0.25, 0.75),
        })
    );
}

#[test]
fn pane_drags_onto_another_zone_tab_move_there() {
    for payload in pane_payloads() {
        assert_eq!(
            resolve_drop(&payload, &DropTarget::Tab(KEYS.to_owned())),
            Some(DropAction::MoveTo(KEYS.to_owned())),
            "{payload:?}"
        );
    }
}

#[test]
fn a_tab_refuses_what_its_zone_already_holds() {
    assert_eq!(
        resolve_drop(&placed_device(), &DropTarget::Tab(MAIN.to_owned())),
        None
    );
    assert_eq!(
        resolve_drop(&placed_channel(), &DropTarget::Tab(MAIN.to_owned())),
        None
    );
    // Unassigned hardware may go to the open zone's own tab.
    assert_eq!(
        resolve_drop(&unassigned_device(), &DropTarget::Tab(MAIN.to_owned())),
        Some(DropAction::MoveTo(MAIN.to_owned()))
    );
}

#[test]
fn pane_drags_onto_plus_make_a_zone() {
    for payload in pane_payloads() {
        assert_eq!(
            resolve_drop(&payload, &DropTarget::NewZone),
            Some(DropAction::NewZone),
            "{payload:?}"
        );
    }
}

#[test]
fn pane_drags_onto_an_other_zones_row_move_there() {
    for payload in pane_payloads() {
        assert_eq!(
            resolve_drop(&payload, &DropTarget::PaneZone(KEYS.to_owned())),
            Some(DropAction::MoveTo(KEYS.to_owned())),
            "{payload:?}"
        );
    }
    assert_eq!(
        resolve_drop(&placed_device(), &DropTarget::PaneZone(MAIN.to_owned())),
        None
    );
}

#[test]
fn pane_drags_onto_unassigned_release_their_zone() {
    assert_eq!(
        resolve_drop(&placed_device(), &DropTarget::Unassigned),
        Some(DropAction::Unassign)
    );
    assert_eq!(
        resolve_drop(&placed_channel(), &DropTarget::Unassigned),
        Some(DropAction::Unassign)
    );
    assert_eq!(
        resolve_drop(&unassigned_device(), &DropTarget::Unassigned),
        None
    );
    assert_eq!(
        resolve_drop(&unassigned_channel(), &DropTarget::Unassigned),
        None
    );
}

#[test]
fn a_canvas_box_drops_only_on_another_zone_tab() {
    let boxes = canvas_boxes();
    assert_eq!(
        resolve_drop(&boxes, &DropTarget::Tab(KEYS.to_owned())),
        Some(DropAction::MoveTo(KEYS.to_owned()))
    );
    assert_eq!(
        resolve_drop(&boxes, &DropTarget::Tab(MAIN.to_owned())),
        None
    );
    assert_eq!(resolve_drop(&boxes, &canvas(MAIN)), None);
    assert_eq!(resolve_drop(&boxes, &DropTarget::NewZone), None);
    assert_eq!(
        resolve_drop(&boxes, &DropTarget::PaneZone(KEYS.to_owned())),
        None
    );
    assert_eq!(resolve_drop(&boxes, &DropTarget::Unassigned), None);
}

#[test]
fn only_pane_drags_spring_load_tabs() {
    for payload in pane_payloads() {
        assert!(springs(&payload), "{payload:?}");
    }
    assert!(!springs(&canvas_boxes()));
}

// ── Payloads ──────────────────────────────────────────────────────────────

#[test]
fn a_channel_in_a_zone_moves_its_output_and_one_in_no_zone_claims_its_segment() {
    let placed = placed_channel();
    assert_eq!(placed.outputs, vec!["ch-4".to_owned()]);
    assert!(placed.segments.is_empty());
    assert_eq!(placed.label, "Channel 4 (Nollie 32)");

    let free = unassigned_channel();
    assert!(free.outputs.is_empty());
    assert_eq!(free.segments, vec!["Channel 6".to_owned()]);
    assert_eq!(free.source, DragSource::Pane);
}

#[test]
fn the_ghost_badges_only_multi_output_payloads() {
    assert_eq!(placed_device().badge(), Some(3));
    assert_eq!(unassigned_device().badge(), Some(2));
    assert_eq!(placed_channel().badge(), None);
    assert_eq!(MovePayload::unassigned_device("a", "A", 1).badge(), None);
    assert_eq!(MovePayload::unassigned_device("a", "A", 0).badge(), None);
}

#[test]
fn channel_labels_name_the_device_unless_redundant() {
    assert_eq!(
        channel_label("ATX Strimer", "Nollie 32"),
        "ATX Strimer (Nollie 32)"
    );
    assert_eq!(channel_label("  ", "Nollie 32"), "Nollie 32");
    assert_eq!(channel_label("Nollie 32", "nollie 32"), "Nollie 32");
    assert_eq!(channel_label("Pads", ""), "Pads");
}

// ── Spring loading ────────────────────────────────────────────────────────

#[test]
fn a_held_tab_opens_after_the_hold() {
    let mut spring = SpringLoad::default();
    assert_eq!(
        spring.hover(Some(KEYS), Some(MAIN), 1_000.0),
        SpringPhase::Charging(KEYS.to_owned())
    );
    assert_eq!(spring.charging(), Some(KEYS));
    assert_eq!(
        spring.hover(Some(KEYS), Some(MAIN), 1_200.0),
        SpringPhase::Holding
    );
    assert_eq!(spring.tick(1_000.0 + SpringLoad::HOLD_MS - 1.0), None);
    assert!((spring.progress(1_250.0) - 0.5).abs() < 1e-9);
    assert_eq!(
        spring.tick(1_000.0 + SpringLoad::HOLD_MS),
        Some(KEYS.to_owned())
    );
    assert_eq!(spring.charging(), None);
    // It fires once.
    assert_eq!(spring.tick(5_000.0), None);
}

#[test]
fn leaving_the_tab_cancels_the_hold() {
    let mut spring = SpringLoad::default();
    spring.hover(Some(KEYS), Some(MAIN), 0.0);
    assert_eq!(spring.hover(None, Some(MAIN), 300.0), SpringPhase::Idle);
    assert_eq!(spring.tick(600.0), None);
    assert_eq!(spring.progress(600.0), 0.0);
}

#[test]
fn moving_to_another_tab_restarts_the_clock() {
    let mut spring = SpringLoad::default();
    spring.hover(Some(KEYS), Some(MAIN), 0.0);
    assert_eq!(
        spring.hover(Some("roli"), Some(MAIN), 400.0),
        SpringPhase::Charging("roli".to_owned())
    );
    assert_eq!(spring.tick(600.0), None);
    assert_eq!(spring.tick(900.0), Some("roli".to_owned()));
}

#[test]
fn the_open_tab_never_charges() {
    let mut spring = SpringLoad::default();
    assert_eq!(spring.hover(Some(MAIN), Some(MAIN), 0.0), SpringPhase::Idle);
    assert_eq!(spring.tick(10_000.0), None);
}

#[test]
fn an_opened_tab_waits_for_the_pointer_to_leave_before_charging_again() {
    let mut spring = SpringLoad::default();
    spring.hover(Some(KEYS), Some(MAIN), 0.0);
    assert_eq!(spring.tick(500.0), Some(KEYS.to_owned()));
    // The selection has not caught up yet; the tab must not recharge.
    assert_eq!(
        spring.hover(Some(KEYS), Some(MAIN), 520.0),
        SpringPhase::Idle
    );
    assert_eq!(spring.tick(1_200.0), None);
    // Leaving and coming back holds again.
    spring.hover(None, Some(KEYS), 1_300.0);
    assert_eq!(
        spring.hover(Some(KEYS), Some(MAIN), 1_400.0),
        SpringPhase::Charging(KEYS.to_owned())
    );
}

// ── New zone naming ───────────────────────────────────────────────────────

#[test]
fn a_zone_from_a_device_takes_its_name() {
    assert_eq!(
        zone_name_for_device("Razer Huntsman V2", &["Main", "ROLI"]),
        "Razer Huntsman V2"
    );
    assert_eq!(
        zone_name_for_device("  Corsair K70  ", &["Main"]),
        "Corsair K70"
    );
}

#[test]
fn a_taken_device_name_is_numbered_case_insensitively() {
    assert_eq!(
        zone_name_for_device("Razer Huntsman V2", &["Main", "razer huntsman v2"]),
        "Razer Huntsman V2 2"
    );
    assert_eq!(
        zone_name_for_device("Push 2", &["Push 2", "Push 2 2", "push 2 3", "Main"]),
        "Push 2 4"
    );
}

#[test]
fn a_nameless_device_falls_back_to_new_zone() {
    assert_eq!(zone_name_for_device("", &["Main"]), "New zone");
    assert_eq!(zone_name_for_device("  ", &["New zone"]), "New zone 2");
}

// ── Announcements ─────────────────────────────────────────────────────────

#[test]
fn moves_announce_name_and_both_places() {
    assert_eq!(
        moved_announcement("Razer Huntsman V2", "Keyboard", "Main"),
        "Moved Razer Huntsman V2 to Keyboard from Main"
    );
    assert_eq!(
        moved_announcement("Channel 4 (Nollie 32)", "Unassigned", "Main"),
        "Moved Channel 4 (Nollie 32) to Unassigned from Main"
    );
    assert_eq!(
        new_zone_announcement("Razer Huntsman V2", "Main"),
        "Moved Razer Huntsman V2 to a new zone from Main"
    );
    assert_eq!(
        placed_announcement("Razer Huntsman V2", "Main"),
        "Placed Razer Huntsman V2 on Main"
    );
    assert_eq!(opened_announcement("Keyboard"), "Opened Keyboard");
    assert_eq!(CANCELLED_ANNOUNCEMENT, "Move cancelled");
}

#[test]
fn origins_announce_by_zone_name_or_unassigned() {
    let names = |id: &str| {
        if id == MAIN {
            "Main".to_owned()
        } else {
            id.to_owned()
        }
    };
    assert_eq!(origin_name(&Origin::Zone(MAIN.to_owned()), names), "Main");
    assert_eq!(origin_name(&Origin::Unassigned, names), "Unassigned");
}

#[test]
fn the_ghost_hint_names_what_a_release_does() {
    let names = |id: &str| {
        if id == KEYS {
            "Keyboard".to_owned()
        } else {
            "Main".to_owned()
        }
    };
    assert_eq!(
        action_hint(&DropAction::MoveTo(KEYS.to_owned()), names),
        "Move to Keyboard"
    );
    assert_eq!(
        action_hint(
            &DropAction::Place {
                zone_id: MAIN.to_owned(),
                point: point(0.5, 0.5),
            },
            names
        ),
        "Place on Main"
    );
    assert_eq!(action_hint(&DropAction::NewZone, names), "New zone");
    assert_eq!(action_hint(&DropAction::Unassign, names), "Unassign");
}
