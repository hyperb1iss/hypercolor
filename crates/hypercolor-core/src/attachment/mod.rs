//! Attachment template registry and bundled catalog loading.

mod embedded;
mod paths;
mod registry;

pub use paths::{bundled_attachments_root, resolve_attachment_path};
pub use registry::{AttachmentRegistry, AttachmentRegistryError, TemplateFilter};
