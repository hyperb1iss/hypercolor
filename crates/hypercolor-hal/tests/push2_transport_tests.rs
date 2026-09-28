use hypercolor_hal::drivers::push2::transport::{
    classify_push2_port_for_testing, midi_packet_spacing_for_testing,
    midi_usb_paths_match_for_testing, select_push2_port_identity_for_testing,
};

use std::time::Duration;

#[cfg(target_os = "linux")]
use hypercolor_hal::drivers::push2::transport::{
    midi_usb_path_from_sound_card_sysfs_for_testing,
    rawmidi_name_from_sound_card_and_seq_port_for_testing, rawmidi_open_retry_for_testing,
    rawmidi_whole_message_write_for_testing, rawmidi_write_deadline_for_testing,
};

#[test]
fn classify_push2_port_recognizes_linux_and_macos_names() {
    assert_eq!(
        classify_push2_port_for_testing("Ableton Push 2 24:0"),
        Some("live")
    );
    assert_eq!(
        classify_push2_port_for_testing("Ableton Push 2 24:1"),
        Some("user")
    );
    assert_eq!(
        classify_push2_port_for_testing("Ableton Push 2 User Port"),
        Some("user")
    );
    assert_eq!(
        classify_push2_port_for_testing("Ableton Push 2 Live Port"),
        Some("live")
    );
    assert_eq!(
        classify_push2_port_for_testing("Ableton Push 2"),
        Some("live")
    );
    assert_eq!(
        classify_push2_port_for_testing("MIDIIN2 (Ableton Push 2)"),
        Some("user")
    );
    assert_eq!(
        classify_push2_port_for_testing("MIDIOUT2 (Ableton Push 2)"),
        Some("user")
    );
    assert_eq!(
        classify_push2_port_for_testing("Unrelated Controller"),
        None
    );
}

#[test]
fn push2_port_selection_prefers_requested_usb_path_when_multiple_match() {
    let selected = select_push2_port_identity_for_testing(
        &[
            ("Ableton Push 2 24:1", "24:1", Some("1-2")),
            ("Ableton Push 2 28:1", "28:1", Some("1-6.3")),
        ],
        "user",
        Some("01-6.3"),
    )
    .expect("USB path filtering should disambiguate the user port");

    assert_eq!(selected, "28:1");
}

#[test]
fn usb_path_matching_normalizes_bus_numbers() {
    assert!(midi_usb_paths_match_for_testing("01-6.3", "1-6.3"));
    assert!(midi_usb_paths_match_for_testing("1-6.3", "01-6.3"));
    assert!(!midi_usb_paths_match_for_testing("1-6.3", "1-6.4"));
}

#[test]
fn midi_packet_spacing_paces_sysex_more_than_short_led_updates() {
    assert_eq!(
        midi_packet_spacing_for_testing(3),
        Duration::from_micros(500)
    );
    assert_eq!(midi_packet_spacing_for_testing(9), Duration::from_millis(1));
    assert_eq!(
        midi_packet_spacing_for_testing(17),
        Duration::from_millis(1)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn rawmidi_write_deadline_times_out_when_device_stops_draining() {
    let result = rawmidi_write_deadline_for_testing(
        &[Err(std::io::ErrorKind::WouldBlock)],
        false,
        Duration::from_secs(1),
        6,
    );

    assert_eq!(result, Err("timeout".to_owned()));
}

#[cfg(target_os = "linux")]
#[test]
fn rawmidi_write_deadline_times_out_after_repeated_spurious_polls() {
    let result = rawmidi_write_deadline_for_testing(
        &[Err(std::io::ErrorKind::WouldBlock)],
        true,
        Duration::from_millis(500),
        6,
    );

    assert_eq!(result, Err("timeout".to_owned()));
}

#[cfg(target_os = "linux")]
#[test]
fn rawmidi_write_deadline_completes_across_partial_writes() {
    let result = rawmidi_write_deadline_for_testing(
        &[
            Ok(2),
            Err(std::io::ErrorKind::WouldBlock),
            Ok(1),
            Err(std::io::ErrorKind::Interrupted),
            Ok(usize::MAX),
        ],
        true,
        Duration::from_secs(1),
        6,
    );

    assert_eq!(result, Ok(()));
}

#[cfg(target_os = "linux")]
#[test]
fn sound_card_sysfs_path_extracts_usb_path() {
    let usb_path = midi_usb_path_from_sound_card_sysfs_for_testing(
        "/devices/pci0000:00/0000:00:14.0/usb1/1-12/1-12:1.1/sound/card4",
    );

    assert_eq!(usb_path.as_deref(), Some("1-12"));
}

#[cfg(target_os = "linux")]
#[test]
fn rawmidi_name_maps_alsa_seq_port_to_user_subdevice() {
    assert_eq!(
        rawmidi_name_from_sound_card_and_seq_port_for_testing(3, 1).as_deref(),
        Some("hw:3,0,1")
    );
    assert_eq!(
        rawmidi_name_from_sound_card_and_seq_port_for_testing(3, -1),
        None
    );
}

#[cfg(target_os = "linux")]
#[test]
fn rawmidi_open_retry_waits_for_hotplug_device_node() {
    let (attempts, elapsed) =
        rawmidi_open_retry_for_testing(2, Duration::from_secs(1), Duration::from_millis(50))
            .expect("rawmidi retry should eventually succeed");

    assert_eq!(attempts, 3);
    assert_eq!(elapsed, Duration::from_millis(100));
}

#[cfg(target_os = "linux")]
#[test]
fn rawmidi_write_never_starts_a_message_the_buffer_cannot_hold() {
    // A wedged endpoint leaves 10 bytes free; a 17-byte palette sysex must
    // time out without a single byte reaching the kernel, so no truncated
    // sysex is ever left on the wire.
    let (written, result) =
        rawmidi_whole_message_write_for_testing(&[10], Duration::from_secs(1), 17);

    assert_eq!(result, Err("timeout".to_owned()));
    assert_eq!(written, 0);
}

#[cfg(target_os = "linux")]
#[test]
fn rawmidi_write_admits_the_whole_message_once_space_frees_up() {
    let (written, result) =
        rawmidi_whole_message_write_for_testing(&[4, 8, 16, 4096], Duration::from_secs(1), 17);

    assert_eq!(result, Ok(()));
    assert_eq!(written, 17);
}

#[cfg(target_os = "linux")]
#[test]
fn rawmidi_write_goes_straight_through_with_room_to_spare() {
    let (written, result) =
        rawmidi_whole_message_write_for_testing(&[4096], Duration::from_millis(1), 3);

    assert_eq!(result, Ok(()));
    assert_eq!(written, 3);
}

mod stall_detection {
    use std::time::Duration;

    use hypercolor_hal::drivers::push2::transport::{
        Push2StallStepForTesting as Step, push2_stall_reports_for_testing as reports,
    };

    fn deadline(at_secs: u64, backlog: Option<usize>) -> Step {
        Step::DeadlinePassed {
            at: Duration::from_secs(at_secs),
            backlog,
        }
    }

    fn reply(at_secs: u64) -> Step {
        Step::ReplyArrived {
            at: Duration::from_secs(at_secs),
        }
    }

    #[test]
    fn a_deadline_with_bytes_stuck_in_the_kernel_names_the_stall() {
        // The field signature: the 6-byte device inquiry still sits in the
        // kernel's rawmidi buffer after the 1 s reply deadline.
        assert_eq!(
            reports("stall-names", &[deadline(1, Some(6))]),
            vec![Some("stalled queued=6".to_owned())]
        );
    }

    #[test]
    fn a_deadline_the_kernel_drained_is_not_called_a_stall() {
        // The bytes left the rawmidi buffer, or the output cannot say: a
        // missing reply alone does not show that output stopped.
        assert_eq!(
            reports("stall-drained", &[deadline(1, Some(0)), deadline(2, None)]),
            vec![None, None]
        );
    }

    #[test]
    fn every_reconnect_attempt_against_a_stalled_device_names_it() {
        // Discovery retries a stalled Push 2 every 5 minutes; each attempt is
        // a new transport that reports once and counts the whole episode.
        assert_eq!(
            reports(
                "stall-reconnects",
                &[
                    deadline(1, Some(6)),
                    Step::Reopened,
                    deadline(301, Some(6)),
                    Step::Reopened,
                    deadline(601, Some(6)),
                ]
            ),
            vec![
                Some("stalled queued=6".to_owned()),
                None,
                Some("still stalled queued=6 for=300s checks=2".to_owned()),
                None,
                Some("still stalled queued=6 for=600s checks=3".to_owned()),
            ]
        );
    }

    #[test]
    fn a_stall_within_one_session_repeats_only_after_the_report_interval() {
        // In-session probes back off from 250 ms; they must not turn the
        // report into a log flood.
        assert_eq!(
            reports(
                "stall-session",
                &[
                    deadline(1, Some(100)),
                    deadline(2, Some(109)),
                    deadline(60, Some(118)),
                    deadline(302, Some(127)),
                ]
            ),
            vec![
                Some("stalled queued=100".to_owned()),
                None,
                None,
                Some("still stalled queued=127 for=301s checks=4".to_owned()),
            ]
        );
    }

    #[test]
    fn the_first_reply_after_a_power_cycle_reports_flowing_once() {
        // A power cycle re-enumerates the device and the next connect's
        // identity reply is the first answer; the episode then closes, and a
        // later stall is a new one.
        assert_eq!(
            reports(
                "stall-recovers",
                &[
                    deadline(1, Some(6)),
                    Step::Reopened,
                    reply(3_601),
                    reply(3_602),
                    deadline(4_000, Some(9)),
                ]
            ),
            vec![
                Some("stalled queued=6".to_owned()),
                None,
                Some("flowing after=3600s checks=1".to_owned()),
                None,
                Some("stalled queued=9".to_owned()),
            ]
        );
    }

    #[test]
    fn replies_from_a_healthy_device_report_nothing() {
        assert_eq!(
            reports("stall-healthy", &[reply(1), reply(2)]),
            vec![None, None]
        );
    }
}
