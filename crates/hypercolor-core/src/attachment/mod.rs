//! Attachment template registry and bundled catalog loading.

mod embedded;
mod paths;
mod profile;
mod registry;

pub use paths::{bundled_attachments_root, resolve_attachment_path};
pub use profile::{effective_attachment_slots, normalize_attachment_profile_slots};
pub use registry::{ComponentRegistry, ComponentRegistryError, TemplateFilter};

// ── Plan 55 P3 backwards-compat aliases ─────────────────────────────────
//
// These match the legacy `AttachmentRegistry*` names so the daemon and
// other consumers keep compiling while the cascade unrolls. Removed
// once every consumer migrates to the `Component*` vocabulary.

/// Deprecated alias for [`ComponentRegistry`]; remove after Plan 55 P3
/// finishes.
pub type AttachmentRegistry = ComponentRegistry;

/// Deprecated alias for [`ComponentRegistryError`]; remove after Plan
/// 55 P3 finishes.
pub type AttachmentRegistryError = ComponentRegistryError;
