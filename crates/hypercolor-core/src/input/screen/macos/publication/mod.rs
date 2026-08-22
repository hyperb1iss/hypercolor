#[cfg(feature = "macos-capture-fixtures")]
mod fixtures;
mod metadata;
mod model;
mod native;
mod resolution;

#[cfg(all(test, feature = "macos-capture-fixtures"))]
pub(super) use fixtures::{
    legacy_analysis_decimation, native_cpu_capture_frame, publish_macos_scalar_exact,
};
pub(super) use metadata::capture_source_id;
#[cfg(all(test, feature = "macos-capture-fixtures"))]
pub(super) use metadata::{capture_colorimetry, capture_pixel_format};
#[cfg(all(test, feature = "macos-capture-fixtures"))]
pub(super) use model::bind_current_macos_exact_runtime;
pub(super) use native::publish_frame;
#[cfg(all(test, feature = "macos-capture-fixtures"))]
pub(super) use native::publish_macos_native_exact;
pub(super) use resolution::resolve_macos_publication_branch_with_telemetry;
#[cfg(all(test, feature = "macos-capture-fixtures"))]
pub(super) use resolution::{
    macos_native_descriptor_is_identity, resolve_macos_publication_branch,
};
