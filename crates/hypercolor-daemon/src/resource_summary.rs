//! Resource summaries every transport renders the same way.
//!
//! REST, MCP tools, and MCP resources all describe effects and scenes
//! through these projections, so a field added here reaches every
//! surface at once instead of drifting per adapter.

use std::path::PathBuf;

use hypercolor_types::api::effects::{EffectCapabilitySet, EffectSourceKind, EffectSummary};
use hypercolor_types::api::scenes::SceneSummary;
use hypercolor_types::effect::{EffectMetadata, EffectSource};
use hypercolor_types::scene::Scene;

use crate::domain::DomainError;

pub(crate) const EFFECT_COVER_FILE_NAME: &str = "default.webp";

/// Which summary expansions a listing asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct EffectListIncludes {
    pub(crate) controls: bool,
    pub(crate) presets: bool,
}

impl EffectListIncludes {
    pub(crate) fn parse(raw: Option<&str>) -> Result<Self, DomainError> {
        let mut includes = Self::default();
        for token in raw.unwrap_or_default().split(',') {
            match token.trim() {
                "" => {}
                "controls" => includes.controls = true,
                "presets" => includes.presets = true,
                other => {
                    return Err(DomainError::validation_field(
                        "include",
                        format!("unknown expansion '{other}'; expected controls or presets"),
                    ));
                }
            }
        }
        Ok(includes)
    }
}

pub(crate) fn effect_summary(meta: &EffectMetadata, includes: EffectListIncludes) -> EffectSummary {
    EffectSummary {
        id: meta.id.to_string(),
        name: meta.name.clone(),
        description: meta.description.clone(),
        author: meta.author.clone(),
        category: meta.category,
        source: EffectSourceKind::from(&meta.source),
        runnable: is_runnable_source(&meta.source),
        tags: meta.tags.clone(),
        version: meta.version.clone(),
        audio_reactive: meta.audio_reactive,
        input_reactive: meta.input_reactive,
        capabilities: EffectCapabilitySet {
            audio_reactive: meta.audio_reactive,
            screen_reactive: meta.screen_reactive,
            input_reactive: meta.input_reactive,
        },
        cover_image_url: effect_cover_image_url(meta),
        controls: includes.controls.then(|| meta.controls.clone()),
        presets: includes.presets.then(|| meta.presets.clone()),
    }
}

pub(crate) fn effect_summary_with_details(meta: &EffectMetadata) -> EffectSummary {
    effect_summary(
        meta,
        EffectListIncludes {
            controls: true,
            presets: true,
        },
    )
}

pub(crate) fn scene_summary(scene: &Scene) -> SceneSummary {
    SceneSummary {
        id: scene.id.to_string(),
        name: scene.name.clone(),
        description: scene.description.clone(),
        enabled: scene.enabled,
        priority: scene.priority.0,
        mutation_mode: scene.mutation_mode,
        layout_id: scene.layout_id.clone(),
        activation_brightness: scene.activation_brightness,
    }
}

pub(crate) fn effect_cover_image_url(metadata: &EffectMetadata) -> Option<String> {
    if effect_cover_image_path(metadata).is_none() && html_effect_source_path(metadata).is_none() {
        return None;
    }
    Some(format!("/api/v1/effects/{}/cover", metadata.id))
}

pub(crate) fn html_effect_source_path(metadata: &EffectMetadata) -> Option<&PathBuf> {
    match &metadata.source {
        EffectSource::Html { path } => Some(path),
        EffectSource::Native { .. } | EffectSource::Shader { .. } => None,
    }
}

pub(crate) fn effect_cover_image_path(metadata: &EffectMetadata) -> Option<PathBuf> {
    let root = hypercolor_core::effect::bundled_screenshots_root();
    effect_cover_slugs(metadata)
        .into_iter()
        .map(|slug| root.join(slug).join(EFFECT_COVER_FILE_NAME))
        .find(|path| path.is_file())
}

fn effect_cover_slugs(metadata: &EffectMetadata) -> Vec<String> {
    let mut slugs = Vec::new();
    if let Some(stem) = metadata.source.source_stem() {
        push_cover_slug(&mut slugs, stem);
    }
    push_cover_slug(&mut slugs, &metadata.name);
    slugs
}

fn push_cover_slug(slugs: &mut Vec<String>, value: &str) {
    let slug = cover_slug(value);
    if !slug.is_empty() && !slugs.iter().any(|existing| existing == &slug) {
        slugs.push(slug);
    }
}

fn cover_slug(value: &str) -> String {
    let mut slug = String::new();
    let mut last_was_separator = false;

    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            last_was_separator = false;
        } else if !slug.is_empty() && !last_was_separator {
            slug.push('-');
            last_was_separator = true;
        }
    }

    if last_was_separator {
        let _ = slug.pop();
    }
    slug
}

pub(crate) fn is_runnable_source(source: &EffectSource) -> bool {
    match source {
        EffectSource::Native { .. } => true,
        EffectSource::Html { .. } => cfg!(feature = "servo"),
        EffectSource::Shader { .. } => false,
    }
}
