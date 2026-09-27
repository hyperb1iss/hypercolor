//! Push 2 LED flow control against a fake device on a simulated clock.

#[path = "support/push2_fake.rs"]
mod push2_fake;

use std::time::Duration;

use hypercolor_hal::protocol::{Protocol, ResponseTolerance};

use push2_fake::{
    ENDPOINT_PACKET_BYTES, KERNEL_BUFFER_BYTES, LED_COUNT, Push2Rig, RGB_LED_COUNT, rainbow_sweep,
    static_gradient, wire_bytes,
};

fn is_ack_request(bytes: &[u8]) -> bool {
    bytes.len() == 9 && bytes[..7] == [0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x04]
}

fn is_reapply(bytes: &[u8]) -> bool {
    bytes == [0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x05, 0xF7]
}

fn is_mode_assert(bytes: &[u8]) -> bool {
    bytes == [0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x0A, 0x01, 0xF7]
}

fn assert_rgb_leds_show(rig: &Push2Rig, frame: &[[u8; 3]]) {
    for (led, expected) in frame.iter().enumerate().take(RGB_LED_COUNT) {
        assert_eq!(
            rig.device.lit_rgb(led),
            *expected,
            "RGB LED {led} should show the frame color"
        );
    }
}

/// Pump one frame until the protocol has nothing left to send for it.
fn settle(rig: &mut Push2Rig, frame: &[[u8; 3]]) -> usize {
    for pass in 1..=64 {
        rig.clock.advance(push2_fake::FRAME_PERIOD);
        if rig.pump(frame).commands == 0 {
            return pass;
        }
    }
    panic!("frame did not settle within 64 passes");
}

#[test]
fn unchanged_frames_send_nothing_once_acknowledged() {
    let mut rig = Push2Rig::connected();
    let frame = static_gradient();
    settle(&mut rig, &frame);
    assert_rgb_leds_show(&rig, &frame);

    let before = rig.device.sent.len();
    for _ in 0..120 {
        rig.clock.advance(push2_fake::FRAME_PERIOD);
        let pass = rig.pump(&frame);
        assert_eq!(
            pass.commands, 0,
            "an unchanged frame must not touch the wire"
        );
    }
    assert_eq!(rig.device.sent.len(), before);
}

#[test]
fn single_pad_change_to_a_known_color_sends_one_message() {
    let mut rig = Push2Rig::connected();
    let mut frame = vec![[0_u8; 3]; LED_COUNT];
    for (index, color) in frame.iter_mut().enumerate().take(64) {
        *color = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0]][index % 4];
    }
    settle(&mut rig, &frame);

    // Pad 10 takes pad 3's color, which already has a palette slot.
    frame[10] = frame[3];
    rig.clock.advance(push2_fake::FRAME_PERIOD);
    let since = rig.clock.elapsed();
    let pass = rig.pump(&frame);

    let sent = rig.sent_since(since);
    assert_eq!(pass.commands, 1);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].bytes, vec![0x90, 36 + 10, rig.device.led_index[3]]);
    assert!(!sent[0].expects_response);
    assert_eq!(rig.device.lit_rgb(10), frame[3]);
}

#[test]
fn single_pad_change_to_a_new_color_stays_within_three_messages() {
    let mut rig = Push2Rig::connected();
    let mut frame = vec![[0_u8; 3]; LED_COUNT];
    for (index, color) in frame.iter_mut().enumerate().take(64) {
        *color = [[255, 0, 0], [0, 255, 0]][index % 2];
    }
    settle(&mut rig, &frame);

    frame[10] = [12, 34, 56];
    rig.clock.advance(push2_fake::FRAME_PERIOD);
    let since = rig.clock.elapsed();
    let pass = rig.pump(&frame);

    // Palette write, Reapply, and the pad's note: no acknowledgement request
    // for a trickle this small.
    assert_eq!(pass.commands, 3);
    assert!(
        rig.sent_since(since)
            .iter()
            .all(|sent| !is_ack_request(&sent.bytes))
    );
    assert_eq!(rig.device.lit_rgb(10), [12, 34, 56]);
}

#[test]
fn full_color_sweep_never_exceeds_one_endpoint_packet_unconfirmed() {
    let mut rig = Push2Rig::connected();
    let sweep_start = rig.clock.elapsed();
    rig.run_for(Duration::from_secs(10), rainbow_sweep);

    let sent = rig.sent_since(sweep_start);
    let total_wire: usize = sent.iter().map(|sent| wire_bytes(&sent.bytes)).sum();
    let total_midi: usize = sent.iter().map(|sent| sent.bytes.len()).sum();
    let acks = sent
        .iter()
        .filter(|sent| is_ack_request(&sent.bytes))
        .count();
    let reapplies = sent.iter().filter(|sent| is_reapply(&sent.bytes)).count();
    let seconds = 10.0;
    #[expect(clippy::cast_precision_loss, reason = "byte counts are small")]
    let (wire_rate, midi_rate) = (total_wire as f64 / seconds, total_midi as f64 / seconds);
    println!(
        "rainbow sweep: {} messages, {total_midi} MIDI bytes ({midi_rate:.0} B/s), \
         {total_wire} wire bytes ({wire_rate:.0} B/s), {acks} acknowledged batches, \
         {reapplies} reapplies, max unconfirmed {} wire bytes",
        sent.len(),
        rig.device.max_unconfirmed_wire_bytes
    );

    assert!(
        rig.device.max_unconfirmed_wire_bytes <= ENDPOINT_PACKET_BYTES,
        "unconfirmed LED traffic reached {} wire bytes",
        rig.device.max_unconfirmed_wire_bytes
    );
    // Every batch that needed more room than one packet waited for its
    // acknowledgement, and no batch sent more than one Reapply.
    assert!(
        acks >= 100,
        "a saturated sweep should run on acknowledged batches, got {acks}"
    );
    assert!(reapplies <= acks);
    // Flow control paces the lane to the device, not below it: a sweep that
    // changes every LED every frame still lands dozens of batches a second.
    assert!(acks >= 20 * 10, "only {acks} acknowledged batches in 10 s");

    // Once the effect stops moving, the device settles on the newest frame.
    let still = rainbow_sweep(10_000);
    let passes = settle(&mut rig, &still);
    assert!(passes <= 12, "static frame took {passes} passes to settle");
    assert_rgb_leds_show(&rig, &still);
}

#[test]
fn each_batch_encodes_the_newest_frame_not_a_backlog() {
    let mut rig = Push2Rig::connected();
    let first = rainbow_sweep(0);
    rig.clock.advance(push2_fake::FRAME_PERIOD);
    let _ = rig.pump(&first);

    // Frames published while a batch is on the wire are superseded; the next
    // batch starts from the newest one and the device converges straight to
    // it without ever showing the skipped frames' colors.
    let newest = rainbow_sweep(500);
    settle(&mut rig, &newest);
    assert_rgb_leds_show(&rig, &newest);
}

#[test]
fn stalled_endpoint_holds_output_to_bounded_probes() {
    let mut rig = Push2Rig::connected();
    settle(&mut rig, &static_gradient());
    let capacity_before = rig.commands.capacity();

    rig.device.stalled = true;
    let stall_start = rig.clock.elapsed();
    rig.run_for(Duration::from_mins(1), rainbow_sweep);

    let sent = rig.sent_since(stall_start);
    let probes = sent
        .iter()
        .filter(|sent| is_ack_request(&sent.bytes))
        .count();
    let led_bytes: usize = sent
        .iter()
        .filter(|sent| !is_ack_request(&sent.bytes))
        .map(|sent| sent.bytes.len())
        .sum();
    println!(
        "stall: {} messages in 60 s ({probes} probes), backlog {} bytes",
        sent.len(),
        rig.device.backlog_bytes()
    );

    // At most one batch went out before the missing acknowledgement was
    // noticed; after that only probes, backed off to one every few seconds.
    assert!(
        led_bytes <= ENDPOINT_PACKET_BYTES,
        "{led_bytes} LED bytes piled onto a stall"
    );
    assert!((2..=20).contains(&probes), "{probes} probes in 60 s");
    assert!(rig.device.backlog_bytes() <= ENDPOINT_PACKET_BYTES + probes * 9);
    assert!(rig.device.backlog_bytes() < KERNEL_BUFFER_BYTES);
    // Nothing grows while the device is gone: the reusable command buffer
    // never needs more room than a normal batch.
    assert!(rig.commands.capacity() <= capacity_before.max(160));
}

#[test]
fn recovery_after_stall_resyncs_to_the_latest_frame() {
    let mut rig = Push2Rig::connected();
    settle(&mut rig, &static_gradient());

    rig.device.stalled = true;
    rig.run_for(Duration::from_secs(20), rainbow_sweep);

    rig.device.unstall();
    let latest = rainbow_sweep(123_456);
    let recovery_start = rig.clock.elapsed();
    for _ in 0..600 {
        rig.clock.advance(push2_fake::FRAME_PERIOD);
        let _ = rig.pump(&latest);
        if (0..RGB_LED_COUNT).all(|led| rig.device.lit_rgb(led) == latest[led]) {
            break;
        }
    }
    let recovered_in = rig.clock.elapsed().saturating_sub(recovery_start);
    println!("recovered in {recovered_in:?}");

    assert_rgb_leds_show(&rig, &latest);
    // The first probe after the stall lands within the probe backoff cap.
    assert!(
        recovered_in <= Duration::from_secs(8),
        "recovery took {recovered_in:?}"
    );
    assert_eq!(
        rig.device.midi_mode, 1,
        "User mode is re-asserted after recovery"
    );
}

#[test]
fn periodic_resync_heals_a_dropped_message() {
    let mut rig = Push2Rig::connected();
    let mut frame = vec![[0_u8; 3]; LED_COUNT];
    for (index, color) in frame.iter_mut().enumerate().take(64) {
        *color = [[255, 0, 0], [0, 255, 0]][index % 2];
    }
    settle(&mut rig, &frame);

    // Pad 5 leaves the shared green slot for the red one, which takes a
    // single note, and an overrun OS buffer eats that note.
    frame[5] = [255, 0, 0];
    rig.device.drop_next_led_message = true;
    rig.clock.advance(push2_fake::FRAME_PERIOD);
    let pass = rig.pump(&frame);
    assert_eq!(pass.commands, 1);
    assert_eq!(
        rig.device.lit_rgb(5),
        [0, 255, 0],
        "the dropped note leaves pad 5 green"
    );

    // Unchanged frames do not heal it on their own...
    rig.run_for(Duration::from_secs(8), |_| frame.clone());
    assert_eq!(rig.device.lit_rgb(5), [0, 255, 0]);

    // ...the periodic resync does, without disturbing any other LED.
    rig.run_for(Duration::from_secs(4), |_| frame.clone());
    assert_rgb_leds_show(&rig, &frame);
}

#[test]
fn user_mode_is_reasserted_inside_the_led_stream() {
    let mut rig = Push2Rig::connected();
    assert!(rig.protocol.keepalive().is_none());

    let frame = static_gradient();
    let start = rig.clock.elapsed();
    rig.run_for(Duration::from_secs(12), |_| frame.clone());

    let asserts: Vec<_> = rig
        .sent_since(start)
        .into_iter()
        .filter(|sent| is_mode_assert(&sent.bytes))
        .collect();
    assert_eq!(
        asserts.len(),
        2,
        "User mode should be re-asserted every 5 s"
    );
    assert!(
        asserts
            .iter()
            .all(|sent| sent.expects_response && sent.optional_response)
    );
    assert_eq!(rig.device.midi_mode, 1);
}

#[test]
fn acknowledgement_requests_wait_for_their_reply_without_failing_a_quiet_device() {
    let mut rig = Push2Rig::connected();
    let before = rig.device.sent.len();
    rig.clock.advance(push2_fake::FRAME_PERIOD);
    let _ = rig.pump(&rainbow_sweep(0));

    let requests: Vec<_> = rig.device.sent[before..]
        .iter()
        .filter(|sent| is_ack_request(&sent.bytes))
        .collect();
    assert_eq!(requests.len(), 1, "a saturated batch ends with one request");
    assert!(requests[0].expects_response && requests[0].optional_response);
    assert!(
        rig.device
            .sent
            .last()
            .is_some_and(|sent| is_ack_request(&sent.bytes)),
        "the request closes the batch"
    );

    let mut commands = Vec::new();
    rig.protocol
        .encode_frame_into(&rainbow_sweep(1), &mut commands);
    assert!(
        commands
            .iter()
            .filter(|command| command.expects_response)
            .all(|command| command.response.tolerance == ResponseTolerance::Optional)
    );
}

#[test]
fn stale_palette_replies_never_rewrite_the_factory_palette() {
    let mut rig = Push2Rig::connected();
    let factory_slot_7 = rig.device.palette[7];
    settle(&mut rig, &static_gradient());

    // A late reply to an abandoned request carries a color the host wrote,
    // not the factory one; it must not leak into the shutdown restore.
    let mut stale = vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x04, 7];
    for value in [1_u8, 2, 3, 4] {
        stale.extend_from_slice(&[value & 0x7F, value >> 7]);
    }
    stale.push(0xF7);
    rig.protocol
        .parse_response(&stale)
        .expect("stale palette reply still parses");

    let shutdown = rig.protocol.shutdown_sequence();
    rig.deliver(&shutdown).expect("shutdown delivers");
    assert_eq!(rig.device.palette[7], factory_slot_7);
}

#[test]
fn palette_writes_stay_budgeted_per_batch_and_converge() {
    let mut rig = Push2Rig::connected();
    let mut frame = vec![[0_u8; 3]; LED_COUNT];
    for (index, color) in frame.iter_mut().enumerate().take(64) {
        *color = [
            u8::try_from(index).expect("pad index fits in u8") + 1,
            40,
            200,
        ];
    }

    let mut writes_per_pass = Vec::new();
    for _ in 0..8 {
        rig.clock.advance(push2_fake::FRAME_PERIOD);
        let since = rig.clock.elapsed();
        let _ = rig.pump(&frame);
        let writes = rig
            .sent_since(since)
            .iter()
            .filter(|sent| sent.bytes.len() == 17 && sent.bytes[6] == 0x03)
            .count();
        writes_per_pass.push(writes);
    }

    assert_eq!(writes_per_pass, vec![16, 16, 16, 16, 0, 0, 0, 0]);
    assert_rgb_leds_show(&rig, &frame);
}
