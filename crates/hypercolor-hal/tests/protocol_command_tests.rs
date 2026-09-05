//! Response-plan defaults and command-buffer slot reuse.

use std::time::Duration;

use hypercolor_hal::protocol::{
    CommandBuffer, ProtocolCommand, ResponsePlan, ResponseTolerance, TransferType,
};

fn planned_command() -> ProtocolCommand {
    ProtocolCommand {
        data: vec![0xAA; 8],
        expects_response: true,
        response_delay: Duration::from_secs(3),
        post_delay: Duration::from_secs(3),
        transfer_type: TransferType::HidReport,
        response: ResponsePlan {
            count: 7,
            timeout: Some(Duration::from_secs(9)),
            capacity: Some(4096),
            tolerance: ResponseTolerance::Optional,
        },
    }
}

#[test]
fn a_default_command_reads_exactly_one_required_report() {
    let command = ProtocolCommand::default();

    assert_eq!(
        command.response,
        ResponsePlan {
            count: 1,
            timeout: None,
            capacity: None,
            tolerance: ResponseTolerance::Required,
        }
    );
    assert!(!command.expects_response);
    assert_eq!(command.transfer_type, TransferType::Primary);
}

/// `CommandBuffer` recycles slots from index zero, so a slot that carried a
/// response plan last frame must come back with the default plan, whichever
/// push path refilled it.
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
    assert_eq!(commands[0].data, vec![0x10]);
    assert_eq!(commands[1].data, vec![0x20]);
    assert_eq!(commands[2].data, vec![0x30, 0x31]);
    for (index, command) in commands.iter().enumerate() {
        assert_eq!(command.transfer_type, TransferType::Bulk);
        assert_eq!(
            command.response,
            ResponsePlan::default(),
            "push path {index} must not inherit the previous frame's plan"
        );
    }
}

/// The pushed slot is handed back so a caller can set a plan on it after the
/// bytes are in place, which is how the display engines apply a per-chunk
/// policy.
#[test]
fn a_pushed_slot_can_carry_a_plan_set_after_the_fill() {
    let mut commands = Vec::new();

    {
        let mut buffer = CommandBuffer::new(&mut commands);
        buffer
            .push_slice(
                &[0x01],
                true,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Bulk,
            )
            .response = ResponsePlan {
            count: 1,
            timeout: Some(Duration::from_secs(2)),
            capacity: Some(511),
            tolerance: ResponseTolerance::Optional,
        };
        buffer.finish();
    }

    assert_eq!(commands[0].response.capacity, Some(511));
    assert_eq!(commands[0].response.tolerance, ResponseTolerance::Optional);
}

#[test]
fn response_plan_builders_set_one_field_each() {
    let counted = ProtocolCommand::default().with_response_count(2);
    assert_eq!(counted.response.count, 2);
    assert_eq!(counted.response.timeout, None);
    assert_eq!(counted.response.capacity, None);
    assert_eq!(counted.response.tolerance, ResponseTolerance::Required);

    let timed = ProtocolCommand::default().with_response_timeout(Duration::from_secs(3));
    assert_eq!(timed.response.timeout, Some(Duration::from_secs(3)));
    assert_eq!(timed.response.count, 1);

    let sized = ProtocolCommand::default().with_response_capacity(508);
    assert_eq!(sized.response.capacity, Some(508));
    assert_eq!(sized.response.count, 1);

    let tolerant = ProtocolCommand::default().with_optional_response();
    assert_eq!(tolerant.response.tolerance, ResponseTolerance::Optional);
    assert_eq!(tolerant.response.count, 1);

    let all = ProtocolCommand::default()
        .with_response_count(2)
        .with_response_timeout(Duration::from_millis(200))
        .with_response_capacity(64)
        .with_optional_response();
    assert_eq!(
        all.response,
        ResponsePlan {
            count: 2,
            timeout: Some(Duration::from_millis(200)),
            capacity: Some(64),
            tolerance: ResponseTolerance::Optional,
        }
    );
}
