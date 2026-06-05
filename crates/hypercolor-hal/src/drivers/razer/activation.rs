//! Razer custom-effect activation policy.

use std::cmp::min;

use crate::protocol::{CommandBuffer, ProtocolCommand};

use super::packet::{self, CommandSpec, CommandTiming};
use super::types::{
    EFFECT_CUSTOM_FRAME, LED_ID_ZERO, NOSTORE, RazerLightingCommandSet, RazerProtocolVersion,
};

// Modern custom-effect activation declares a 6-byte payload even though the
// meaningful arguments only consume 5 bytes.
const EXTENDED_CUSTOM_EFFECT_DATA_SIZE: u8 = 0x06;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CustomEffectActivationStyle {
    MatchCommandSet,
    LegacyStandard {
        storage: u8,
    },
    StandardLedEffect {
        storage: u8,
        led_id: u8,
        effect: u8,
    },
    ExtendedMatrix {
        declared_data_size: u8,
        args: [u8; 5],
        args_len: u8,
    },
}

impl CustomEffectActivationStyle {
    pub(super) fn command(
        self,
        version: RazerProtocolVersion,
        command_set: RazerLightingCommandSet,
        standard_storage: u8,
        timing: CommandTiming,
    ) -> Option<ProtocolCommand> {
        let args = self.command_args(command_set, standard_storage);
        packet::build_command(CommandSpec {
            transaction_id: version.transaction_id(),
            command_class: args.command_class,
            command_id: args.command_id,
            args: args.as_slice(),
            declared_data_size: args.declared_data_size,
            timing,
        })
    }

    pub(super) fn push(
        self,
        encoder: &mut CommandBuffer<'_>,
        version: RazerProtocolVersion,
        command_set: RazerLightingCommandSet,
        standard_storage: u8,
        timing: CommandTiming,
    ) {
        let args = self.command_args(command_set, standard_storage);
        packet::push_command(
            encoder,
            CommandSpec {
                transaction_id: version.transaction_id(),
                command_class: args.command_class,
                command_id: args.command_id,
                args: args.as_slice(),
                declared_data_size: args.declared_data_size,
                timing,
            },
        );
    }

    fn command_args(
        self,
        command_set: RazerLightingCommandSet,
        standard_storage: u8,
    ) -> ActivationCommandArgs {
        match self {
            Self::LegacyStandard { storage } => {
                ActivationCommandArgs::standard_custom(&[0x05, storage])
            }
            Self::StandardLedEffect {
                storage,
                led_id,
                effect,
            } => ActivationCommandArgs::standard_led_effect(&[storage, led_id, effect]),
            Self::ExtendedMatrix {
                declared_data_size,
                args,
                args_len,
            } => {
                let args_len = min(usize::from(args_len), args.len());
                ActivationCommandArgs::extended_matrix(&args[..args_len], declared_data_size)
            }
            Self::MatchCommandSet if matches!(command_set, RazerLightingCommandSet::Standard) => {
                ActivationCommandArgs::standard_custom(&[0x05, standard_storage])
            }
            Self::MatchCommandSet => ActivationCommandArgs::extended_matrix(
                &[NOSTORE, LED_ID_ZERO, EFFECT_CUSTOM_FRAME, 0x00, 0x01],
                EXTENDED_CUSTOM_EFFECT_DATA_SIZE,
            ),
        }
    }
}

struct ActivationCommandArgs {
    command_class: u8,
    command_id: u8,
    data: [u8; 5],
    data_len: usize,
    declared_data_size: Option<u8>,
}

impl ActivationCommandArgs {
    fn standard_custom(data: &[u8]) -> Self {
        Self::new(0x03, 0x0A, data, None)
    }

    fn standard_led_effect(data: &[u8]) -> Self {
        Self::new(0x03, 0x02, data, None)
    }

    fn extended_matrix(data: &[u8], declared_data_size: u8) -> Self {
        Self::new(0x0F, 0x02, data, Some(declared_data_size))
    }

    fn new(
        command_class: u8,
        command_id: u8,
        source: &[u8],
        declared_data_size: Option<u8>,
    ) -> Self {
        debug_assert!(source.len() <= 5);
        let data_len = source.len();
        let mut data = [0_u8; 5];
        data[..data_len].copy_from_slice(&source[..data_len]);
        Self {
            command_class,
            command_id,
            data,
            data_len,
            declared_data_size,
        }
    }

    fn as_slice(&self) -> &[u8] {
        &self.data[..self.data_len]
    }
}
