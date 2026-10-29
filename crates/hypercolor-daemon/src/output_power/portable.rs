//! Scoped publication of portable output settings after durable store writes.

use hypercolor_core::bus::HypercolorBus;
use hypercolor_types::event::HypercolorEvent;
use hypercolor_types::portable_setup::PortableOutputState;
use std::sync::Arc;
use tokio::sync::OwnedMutexGuard;

use super::{OutputOverride, OutputPower, brightness_percent};
use crate::domain::DomainError;

pub(crate) struct PreparedPortablePower {
    power: OutputPower,
    desired: PortableOutputState,
    _guard: OwnedMutexGuard<()>,
}

impl OutputPower {
    pub(crate) async fn prepare_portable_output(
        &self,
        expected: &PortableOutputState,
        desired: PortableOutputState,
    ) -> Result<PreparedPortablePower, DomainError> {
        let guard = Arc::clone(&self.inner.transition).lock_owned().await;
        let current = self.snapshot();
        if current.global_brightness != expected.brightness
            || current.manually_paused() != expected.paused
            || (current.output_override != OutputOverride::Stopped) != expected.enabled
        {
            return Err(DomainError::conflict(
                "output changed before setup restoration",
            ));
        }
        Ok(PreparedPortablePower {
            power: self.clone(),
            desired,
            _guard: guard,
        })
    }

    pub(crate) async fn restore_output_stopped(&self) {
        let _guard = self.inner.transition.lock().await;
        self.update_state(|state| state.output_override = OutputOverride::Stopped);
    }
}

impl PreparedPortablePower {
    pub(crate) fn publish(&self, bus: &HypercolorBus) {
        let previous = self.power.snapshot();
        self.power.update_state_with_events(bus, |state| {
            state.global_brightness = self.desired.brightness;
            state.output_override = if !self.desired.enabled {
                OutputOverride::Stopped
            } else if self.desired.paused {
                OutputOverride::Paused
            } else {
                OutputOverride::None
            };
            state.transition_generation = state.transition_generation.wrapping_add(1);
        });
        if previous.global_brightness != self.desired.brightness {
            bus.publish(HypercolorEvent::BrightnessChanged {
                old: brightness_percent(previous.global_brightness),
                new_value: brightness_percent(self.desired.brightness),
            });
        }
    }
}
