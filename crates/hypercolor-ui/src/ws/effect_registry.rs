//! Invalidation for effect definitions changed by installs or external edits.

use leptos::prelude::*;

/// A registry epoch survives later scene/control messages in the same turn.
#[derive(Clone, Copy)]
pub struct EffectRegistryChanges {
    pub generation: ReadSignal<u64>,
    set_generation: WriteSignal<u64>,
}

impl EffectRegistryChanges {
    pub fn install() -> Self {
        let (generation, set_generation) = signal(0);
        let changes = Self {
            generation,
            set_generation,
        };
        provide_context(changes);
        changes
    }

    /// Observe the daemon's event before dispatching unrelated message state.
    pub fn observe(&self, message: &serde_json::Value) {
        if message.get("type").and_then(serde_json::Value::as_str) == Some("event")
            && message.get("event").and_then(serde_json::Value::as_str)
                == Some("effect_registry_updated")
        {
            self.set_generation
                .update(|generation| *generation = generation.wrapping_add(1));
        }
    }
}
