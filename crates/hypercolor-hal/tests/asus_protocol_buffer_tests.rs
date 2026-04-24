use std::time::Duration;

use hypercolor_hal::drivers::asus::{AuraControllerGen, AuraUsbProtocol};
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, TransferType};

#[test]
fn aura_usb_encode_frame_into_reuses_existing_command_buffers() {
    let protocol = AuraUsbProtocol::new(AuraControllerGen::Motherboard).with_topology(3, vec![30]);
    let colors = vec![[0x10, 0x20, 0x30]; 33];
    let mut commands = vec![ProtocolCommand {
        data: Vec::with_capacity(128),
        expects_response: true,
        response_delay: Duration::from_millis(7),
        post_delay: Duration::from_millis(9),
        transfer_type: TransferType::Bulk,
    }];
    let original_ptr = commands[0].data.as_ptr();

    protocol.encode_frame_into(&colors, &mut commands);

    assert_eq!(commands.len(), 3);
    assert_eq!(commands[0].data.as_ptr(), original_ptr);
    assert!(!commands[0].expects_response);
    assert_eq!(commands[0].transfer_type, TransferType::Primary);
}

#[test]
fn aura_usb_encode_frame_into_overwrites_stale_commands_for_empty_frames() {
    let protocol = AuraUsbProtocol::new(AuraControllerGen::Motherboard).with_topology(3, vec![30]);
    let mut commands = vec![ProtocolCommand {
        data: vec![0xAA],
        expects_response: true,
        response_delay: Duration::from_millis(7),
        post_delay: Duration::from_millis(9),
        transfer_type: TransferType::Bulk,
    }];

    protocol.encode_frame_into(&[], &mut commands);

    assert_eq!(commands.len(), 3);
    assert_ne!(commands[0].data, [0xAA]);
    assert!(!commands[0].expects_response);
    assert_eq!(commands[0].transfer_type, TransferType::Primary);
}
