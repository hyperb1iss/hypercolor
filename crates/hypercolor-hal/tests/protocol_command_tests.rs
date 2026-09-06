//! Response-plan defaults and command-buffer slot reuse.

use std::time::Duration;

use hypercolor_hal::protocol::{
    CommandBuffer, DEFAULT_RESPONSE_COUNT, ProtocolCommand, TransferType,
};

fn planned_command() -> ProtocolCommand {
    ProtocolCommand {
        data: vec![0xAA; 8],
        expects_response: true,
        response_delay: Duration::from_secs(3),
        post_delay: Duration::from_secs(3),
        transfer_type: TransferType::HidReport,
        response_count: 7,
        response_timeout: Some(Duration::from_secs(9)),
        response_len: Some(4096),
    }
}

fn assert_default_plan(command: &ProtocolCommand, label: &str) {
    assert_eq!(
        command.response_count, DEFAULT_RESPONSE_COUNT,
        "{label} should read one response report"
    );
    assert_eq!(
        command.response_timeout, None,
        "{label} should defer to the protocol-wide timeout"
    );
    assert_eq!(
        command.response_len, None,
        "{label} should read once at the transport default"
    );
}

#[test]
fn a_default_command_reads_exactly_one_response_report() {
    let command = ProtocolCommand::default();

    assert_eq!(DEFAULT_RESPONSE_COUNT, 1);
    assert_default_plan(&command, "a default command");
    assert!(!command.expects_response);
    assert_eq!(command.transfer_type, TransferType::Primary);
}

/// `CommandBuffer` recycles slots from index zero, so a slot that carried a
/// response plan last frame must come back with the default plan, not the
/// previous command's.
#[test]
fn command_buffer_clears_the_response_plan_of_a_reused_slot() {
    let mut commands = vec![planned_command()];

    {
        let mut buffer = CommandBuffer::new(&mut commands);
        buffer.push_slice(
            &[0x01, 0x02],
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Bulk,
        );
        buffer.finish();
    }

    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].data, vec![0x01, 0x02]);
    assert_eq!(commands[0].transfer_type, TransferType::Bulk);
    assert_default_plan(&commands[0], "a reused slot");
}

#[test]
fn every_command_buffer_push_path_clears_the_response_plan() {
    let mut commands = vec![planned_command(), planned_command(), planned_command()];

    {
        let mut buffer = CommandBuffer::new(&mut commands);
        buffer.push_fill(
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Bulk,
            |data| data.extend_from_slice(&[0x10]),
        );
        buffer.push_slice(
            &[0x20],
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Bulk,
        );
        buffer.push_struct(
            &[0x30_u8, 0x31],
            false,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Bulk,
        );
        buffer.finish();
    }

    assert_eq!(commands.len(), 3);
    for (index, command) in commands.iter().enumerate() {
        assert_default_plan(command, &format!("push path {index}"));
    }
}

#[test]
fn response_plan_builders_set_one_field_each() {
    let counted = ProtocolCommand::default().with_response_count(2);
    assert_eq!(counted.response_count, 2);
    assert_eq!(counted.response_timeout, None);
    assert_eq!(counted.response_len, None);

    let timed = ProtocolCommand::default().with_response_timeout(Duration::from_secs(3));
    assert_eq!(timed.response_timeout, Some(Duration::from_secs(3)));
    assert_eq!(timed.response_count, DEFAULT_RESPONSE_COUNT);

    let sized = ProtocolCommand::default().with_response_len(508);
    assert_eq!(sized.response_len, Some(508));
    assert_eq!(sized.response_count, DEFAULT_RESPONSE_COUNT);

    let all = ProtocolCommand::default()
        .with_response_count(2)
        .with_response_timeout(Duration::from_millis(200))
        .with_response_len(64);
    assert_eq!(all.response_count, 2);
    assert_eq!(all.response_timeout, Some(Duration::from_millis(200)));
    assert_eq!(all.response_len, Some(64));
}
