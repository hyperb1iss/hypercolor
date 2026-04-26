//! Attachment template registry and bundled catalog loading.

mod embedded;
mod paths;
mod profile;
mod registry;

pub use paths::{bundled_attachments_root, resolve_attachment_path};
pub use profile::{effective_attachment_slots, normalize_attachment_profile_slots};
pub use registry::{AttachmentRegistry, AttachmentRegistryError, TemplateFilter};
