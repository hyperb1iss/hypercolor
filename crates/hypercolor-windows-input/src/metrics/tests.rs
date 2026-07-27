use super::{MonitorTopology, topology_from_monitors};
use crate::decode::ScreenRect;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_UNAWARE};

fn rect(left: i32, top: i32, width: i32, height: i32) -> ScreenRect {
    ScreenRect {
        left,
        top,
        width,
        height,
    }
}

#[test]
fn physical_monitor_union_preserves_negative_origins_and_gaps() {
    let left = rect(-1920, 0, 1920, 1080);
    let primary = rect(0, 200, 2560, 1440);
    let topology = topology_from_monitors(vec![(left, false), (primary, true)])
        .expect("valid monitor topology");

    assert_eq!(topology.primary_screen, primary);
    assert_eq!(topology.virtual_screen, rect(-1920, 0, 4480, 1640));
    assert_eq!(topology.monitors, vec![left, primary]);
}

#[test]
fn physical_monitor_union_rejects_overflowing_geometry() {
    let topology = topology_from_monitors(vec![(rect(i32::MAX, 0, 2, 1), true)]);

    assert_eq!(topology, None);
}

#[test]
fn a_missing_primary_flag_selects_the_first_physical_monitor() {
    let first = rect(100, -500, 800, 600);
    let second = rect(900, 0, 1024, 768);
    let topology = topology_from_monitors(vec![(first, false), (second, false)])
        .expect("valid monitor topology");

    assert_eq!(topology.primary_screen, first);
}

#[test]
fn live_topology_contains_every_physical_monitor() {
    let MonitorTopology {
        monitors,
        virtual_screen,
        primary_screen,
    } = super::physical_monitor_topology().expect("Windows exposes physical monitor topology");

    assert!(!monitors.is_empty());
    assert!(monitors.contains(&primary_screen));
    for monitor in monitors {
        assert!(monitor.left >= virtual_screen.left);
        assert!(monitor.top >= virtual_screen.top);
        assert!(monitor.left + monitor.width <= virtual_screen.left + virtual_screen.width);
        assert!(monitor.top + monitor.height <= virtual_screen.top + virtual_screen.height);
    }
}

#[test]
fn unaware_pseudo_handle_is_a_valid_previous_dpi_context() {
    assert!(super::dpi_context_call_succeeded(
        DPI_AWARENESS_CONTEXT_UNAWARE
    ));
    assert!(!super::dpi_context_call_succeeded(DPI_AWARENESS_CONTEXT(
        std::ptr::null_mut()
    )));
}
