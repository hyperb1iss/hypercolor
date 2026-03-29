use std::time::Duration;

use hypercolor_hal::transport::vendor::{
    VendorControlOperation, decode_operations, encode_operations,
};

#[test]
fn vendor_control_operations_round_trip_through_transport_framing() {
    let operations = vec![
        VendorControlOperation::Write {
            request: 0x80,
            value: 0x0000,
            index: 0xE021,
            data: vec![0x34],
        },
        VendorControlOperation::Delay {
            duration: Duration::from_millis(5),
        },
        VendorControlOperation::Read {
            request: 0x81,
            value: 0x0000,
            index: 0xB500,
            length: 5,
        },
    ];

    let encoded = encode_operations(&operations)
        .expect("vendor operation sequence should encode into transport framing");
    let decoded = decode_operations(&encoded)
        .expect("vendor transport bytes should decode back into operations");

    assert_eq!(decoded, operations);
}
