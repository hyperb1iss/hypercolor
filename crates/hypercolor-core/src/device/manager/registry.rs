use std::sync::Arc;

use tracing::debug;

use super::super::traits::DeviceBackend;
use super::{BackendIo, BackendManager};

impl BackendManager {
    /// Register a device backend. Uses `backend.info().id` as the key.
    ///
    /// Replaces any existing backend with the same ID.
    pub fn register_backend(&mut self, backend: Arc<dyn DeviceBackend>) {
        let info = backend.info();
        let backend_id = info.id.clone();

        debug!(
            backend_id = %backend_id,
            name = %info.name,
            "registered device backend"
        );

        // If a backend gets replaced, drop all output queues bound to that ID.
        // They are lazily recreated on the next frame.
        self.output.remove_backend_state(&backend_id);

        self.backend_generation_counter =
            self.backend_generation_counter.checked_add(1).unwrap_or(1);
        self.backend_generations
            .insert(backend_id.clone(), self.backend_generation_counter);
        self.backends.insert(backend_id, backend);
    }

    /// Clone a backend I/O handle without holding the manager across awaits.
    #[must_use]
    pub fn backend_io(&self, backend_id: &str) -> Option<BackendIo> {
        self.backends.get(backend_id).cloned().map(BackendIo::new)
    }

    /// List registered backend IDs.
    #[must_use]
    pub fn backend_ids(&self) -> Vec<&str> {
        self.backends.keys().map(String::as_str).collect()
    }

    /// Number of registered backends.
    #[must_use]
    pub fn backend_count(&self) -> usize {
        self.backends.len()
    }

    /// Current registration generation for one backend ID.
    #[must_use]
    pub fn backend_generation(&self, backend_id: &str) -> Option<u64> {
        self.backend_generations.get(backend_id).copied()
    }
}
