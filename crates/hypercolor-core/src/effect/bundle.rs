//! Stable identity and entrypoint for a portable custom HTML effect.

use std::fs;
use std::path::{Path, PathBuf};

use hypercolor_types::effect::EffectId;
use serde::{Deserialize, Serialize};

use super::HtmlDiscoveryError;

/// Identity document written beside an imported effect's resources.
pub const HTML_EFFECT_BUNDLE_MANIFEST: &str = "hypercolor-effect.json";

/// Discovery fence retained while a setup restore fills a managed bundle.
pub const HTML_EFFECT_BUNDLE_PENDING: &str = ".hypercolor-effect-pending";

pub(super) fn is_pending(path: &Path) -> bool {
    path.ancestors()
        .any(|parent| parent.join(HTML_EFFECT_BUNDLE_PENDING).exists())
}

/// One custom effect, independent of its installation directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HtmlEffectBundleManifest {
    pub schema_version: u32,
    pub id: EffectId,
    pub entrypoint: String,
}

/// A validated bundle and the directory that owns all its resource paths.
#[derive(Debug, Clone)]
pub struct HtmlEffectBundle {
    pub manifest: HtmlEffectBundleManifest,
    pub root: PathBuf,
    pub entrypoint: PathBuf,
}

impl HtmlEffectBundleManifest {
    /// Validate identity and resolve an existing HTML entrypoint inside `root`.
    pub fn resolve_entrypoint(&self, root: &Path) -> Result<PathBuf, HtmlDiscoveryError> {
        let fail = |message: &str| HtmlDiscoveryError {
            path: root.join(HTML_EFFECT_BUNDLE_MANIFEST),
            message: message.to_owned(),
        };
        if self.schema_version != 1 || self.id.0.is_nil() {
            return Err(fail("unsupported bundle schema or empty effect identity"));
        }
        if super::builtin::is_reserved_effect_id(self.id) {
            return Err(fail("custom effect identity collides with a native effect"));
        }
        if self.entrypoint.contains(['\\', ':'])
            || self.entrypoint.chars().any(char::is_control)
            || self.entrypoint.split('/').any(|part| {
                part.is_empty() || matches!(part, "." | "..") || part.ends_with([' ', '.'])
            })
            || !Path::new(&self.entrypoint)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("html"))
        {
            return Err(fail(
                "bundle entrypoint must be a relative HTML resource path",
            ));
        }
        let root = fs::canonicalize(root).map_err(|error| fail(&error.to_string()))?;
        let entrypoint = fs::canonicalize(root.join(&self.entrypoint))
            .map_err(|error| fail(&format!("cannot resolve bundle entrypoint: {error}")))?;
        if !entrypoint.starts_with(&root) || !entrypoint.is_file() {
            return Err(fail(
                "bundle entrypoint escapes its directory or is not a file",
            ));
        }
        Ok(entrypoint)
    }
}

pub(super) fn is_manifest(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == HTML_EFFECT_BUNDLE_MANIFEST)
}

/// Find the outer bundle owning a source or resource, validating its entrypoint.
/// Exporters use `root` to preserve sibling resources of nested entrypoints.
pub fn html_effect_bundle(path: &Path) -> Result<Option<HtmlEffectBundle>, HtmlDiscoveryError> {
    let absolute = std::path::absolute(path).map_err(|error| HtmlDiscoveryError {
        path: path.to_owned(),
        message: error.to_string(),
    })?;
    let Some(manifest_path) = absolute.parent().and_then(|parent| {
        parent
            .ancestors()
            .map(|root| root.join(HTML_EFFECT_BUNDLE_MANIFEST))
            .filter(|candidate| candidate.exists())
            .last()
    }) else {
        return Ok(None);
    };
    let fail = |message: String| HtmlDiscoveryError {
        path: manifest_path.clone(),
        message,
    };
    let bytes = fs::read(&manifest_path).map_err(|error| fail(error.to_string()))?;
    let manifest: HtmlEffectBundleManifest = serde_json::from_slice(&bytes)
        .map_err(|error| fail(format!("invalid effect bundle manifest: {error}")))?;
    let root = fs::canonicalize(manifest_path.parent().expect("manifest parent"))
        .map_err(|error| fail(error.to_string()))?;
    let entrypoint = manifest.resolve_entrypoint(&root)?;
    Ok(Some(HtmlEffectBundle {
        manifest,
        root,
        entrypoint,
    }))
}
