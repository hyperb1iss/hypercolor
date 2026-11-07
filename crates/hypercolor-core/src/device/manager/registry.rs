use std::sync::Arc;

use hypercolor_driver_api::DeviceBackend;
use tracing::debug;

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

    /// Remove a device backend and every output lane bound to it.
    ///
    /// This only forgets the route. It returns the backend so the caller can
    /// [`DeviceBackend::retire`] it after releasing the manager, which ends
    /// whatever sessions it still holds; returns `None` when nothing was
    /// registered under that ID.
    pub fn unregister_backend(&mut self, backend_id: &str) -> Option<Arc<dyn DeviceBackend>> {
        let backend = self.backends.remove(backend_id)?;
        self.backend_generations.remove(backend_id);
        self.output.remove_backend_state(backend_id);
        debug!(backend_id = %backend_id, "unregistered device backend");
        Some(backend)
    }

    /// Clone a backend I/O handle without holding the manager across awaits.
    #[must_use]
    pub fn backend_io(&self, backend_id: &str) -> Option<BackendIo> {
        self.backends.get(backend_id).cloned().map(BackendIo::new)
    }

    /// Whether `io` still reaches the backend registered under `backend_id`.
    ///
    /// Registering and unregistering a backend both happen under the manager
    /// lock, so a caller holding that lock may publish a connection this
    /// approves. A connection made through a backend retired in the meantime
    /// never reaches a route.
    #[must_use]
    pub fn is_current_backend(&self, backend_id: &str, io: &BackendIo) -> bool {
        self.backends
            .get(backend_id)
            .is_some_and(|backend| io.is_backed_by(backend))
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
