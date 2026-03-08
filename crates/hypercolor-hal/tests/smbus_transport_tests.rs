use std::time::Duration;

use hypercolor_hal::transport::smbus::{SmBusOperation, decode_operations, encode_operations};

#[test]
fn smbus_operation_codec_round_trips() {
    let operations = vec![
        SmBusOperation::WriteWordData {
            register: 0x00,
            value: 0xA080,
        },
        SmBusOperation::Delay {
            duration: Duration::from_millis(1),
        },
        SmBusOperation::WriteByteData {
            register: 0x01,
            value: 0x01,
        },
        SmBusOperation::ReadByteData { register: 0x81 },
        SmBusOperation::WriteBlockData {
            register: 0x03,
            data: vec![1, 2, 3],
        },
    ];

    let encoded = encode_operations(&operations).expect("operations should encode");
    let decoded = decode_operations(&encoded).expect("operations should decode");
    assert_eq!(decoded, operations);
}

#[test]
fn smbus_decode_rejects_unknown_opcode() {
    let error = decode_operations(&[0xFF]).expect_err("unknown opcode should fail");
    let rendered = error.to_string();
    assert!(rendered.contains("unknown SMBus opcode"));
}

#[test]
fn smbus_decode_rejects_truncated_block_payload() {
    let error =
        decode_operations(&[0x04, 0x03, 0x03, 0xAA]).expect_err("truncated block should fail");
    let rendered = error.to_string();
    assert!(rendered.contains("truncated"));
}

#[test]
fn smbus_encode_rejects_delays_outside_u16_millis() {
    let operations = vec![SmBusOperation::Delay {
        duration: Duration::from_secs(70),
    }];

    let error = encode_operations(&operations).expect_err("long delay should fail");
    let rendered = error.to_string();
    assert!(rendered.contains("delay"));
}
