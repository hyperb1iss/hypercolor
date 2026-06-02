//! Leptos-free vocabulary for the layer panel: blend/fit option lists,
//! source labels, and the Add-layer picker model.
//!
//! Deliberately free of `leptos` and `crate::` paths so the layer-panel
//! contract is exercisable from `tests/layer_panel_tests.rs` via a
//! `#[path]` include, mirroring `route_ui.rs` and `label_utils.rs`.

use std::collections::HashMap;

use hypercolor_types::asset::AssetId;
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::{LayerBlendMode, LayerSource, MediaPlayback};
use hypercolor_types::scene::{Zone, ZoneRole};
use hypercolor_types::viewport::FitMode;
use uuid::Uuid;

/// Content sources exposed directly by the Add-layer picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerSourceKind {
    Effect,
    Media,
}

impl LayerSourceKind {
    /// Tab order for the picker.
    pub const ALL: [Self; 2] = [Self::Effect, Self::Media];

    /// User-facing tab label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Effect => "Effect",
            Self::Media => "Media",
        }
    }
}

/// Which effect category domain the picker should show for the current
/// target surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectPickerMode {
    Effects,
    Faces,
    Mixed,
}

impl EffectPickerMode {
    #[must_use]
    pub const fn tab_label(self) -> &'static str {
        match self {
            Self::Faces => "Face",
            Self::Effects | Self::Mixed => "Effect",
        }
    }

    #[must_use]
    pub const fn search_placeholder(self) -> &'static str {
        match self {
            Self::Faces => "Search faces and effects...",
            Self::Effects | Self::Mixed => "Search effects...",
        }
    }

    #[must_use]
    pub const fn empty_detail(self) -> &'static str {
        match self {
            Self::Faces => "No matching faces or effects",
            Self::Effects | Self::Mixed => "No matching effects",
        }
    }

    #[must_use]
    pub fn includes_category(self, category: &str) -> bool {
        let is_display = category.eq_ignore_ascii_case("display");
        match self {
            Self::Effects => !is_display,
            Self::Faces | Self::Mixed => true,
        }
    }

    #[must_use]
    pub fn sort_bucket(self, category: &str) -> u8 {
        if self == Self::Faces && category.eq_ignore_ascii_case("display") {
            0
        } else {
            1
        }
    }
}

/// Where an Add-layer action sends the new layer (§6.6). The spec's
/// *Selected surfaces* scope, which rides the surface-rail multi-select,
/// is deferred until that multi-select lands with multi-zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddLayerScope {
    /// The selected surface only — the default.
    ThisSurface,
    /// Every LED zone.
    AllZones,
    /// Every display-face screen.
    AllScreens,
    /// Every surface in the scene.
    WholeScene,
}

impl AddLayerScope {
    /// User-facing label for the scope selector.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::ThisSurface => "This surface",
            Self::AllZones => "All zones",
            Self::AllScreens => "All screens",
            Self::WholeScene => "Whole scene",
        }
    }
}

#[must_use]
pub fn effect_picker_mode(
    scope: AddLayerScope,
    selected_role: Option<ZoneRole>,
) -> EffectPickerMode {
    match scope {
        AddLayerScope::AllScreens => EffectPickerMode::Faces,
        AddLayerScope::AllZones => EffectPickerMode::Effects,
        AddLayerScope::WholeScene => EffectPickerMode::Mixed,
        AddLayerScope::ThisSurface => {
            if selected_role == Some(ZoneRole::Display) {
                EffectPickerMode::Faces
            } else {
                EffectPickerMode::Effects
            }
        }
    }
}

#[must_use]
pub fn effect_category_label(category: &str) -> String {
    if category.eq_ignore_ascii_case("display") {
        "face".to_owned()
    } else {
        category.to_owned()
    }
}

#[must_use]
pub fn effect_picker_matches_query(name: &str, category: &str, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || name.to_lowercase().contains(&query)
        || category.to_lowercase().contains(&query)
        || effect_category_label(category)
            .to_lowercase()
            .contains(&query)
}

/// The scopes worth offering for a scene. With one surface there is
/// nothing to scope to, and a scope that would target nothing is dropped
/// (§6.6), so a result shorter than two means "show no selector".
#[must_use]
pub fn available_add_layer_scopes(groups: &[Zone]) -> Vec<AddLayerScope> {
    if groups.len() < 2 {
        return Vec::new();
    }
    let has_lights = groups.iter().any(|group| group.role != ZoneRole::Display);
    let has_screens = groups.iter().any(|group| group.role == ZoneRole::Display);
    let mut scopes = vec![AddLayerScope::ThisSurface];
    if has_lights {
        scopes.push(AddLayerScope::AllZones);
    }
    if has_screens {
        scopes.push(AddLayerScope::AllScreens);
    }
    scopes.push(AddLayerScope::WholeScene);
    scopes
}

/// Resolve a scope to the render-group ids that should receive the layer,
/// in scene order. Targets are deduplicated so a scope can never queue the
/// same surface twice.
#[must_use]
pub fn resolve_add_layer_targets(
    scope: AddLayerScope,
    groups: &[Zone],
    selected_group_id: &str,
) -> Vec<String> {
    match scope {
        AddLayerScope::ThisSurface => vec![selected_group_id.to_owned()],
        AddLayerScope::AllZones => groups
            .iter()
            .filter(|group| group.role != ZoneRole::Display)
            .map(|group| group.id.to_string())
            .collect(),
        AddLayerScope::AllScreens => groups
            .iter()
            .filter(|group| group.role == ZoneRole::Display)
            .map(|group| group.id.to_string())
            .collect(),
        AddLayerScope::WholeScene => groups.iter().map(|group| group.id.to_string()).collect(),
    }
}

/// Build an effect layer source from a registry effect id string.
///
/// Effect ids on the wire are UUIDs; a non-UUID string cannot key a layer.
pub fn effect_layer_source(effect_id: &str) -> Result<LayerSource, String> {
    let uuid = Uuid::parse_str(effect_id.trim())
        .map_err(|_| format!("effect id is not a valid identifier: {effect_id}"))?;
    Ok(LayerSource::Effect {
        effect_id: EffectId::new(uuid),
        controls: HashMap::new(),
        control_bindings: HashMap::new(),
        preset_id: None,
    })
}

/// Build a media layer source from an asset id string.
pub fn media_layer_source(asset_id: &str) -> Result<LayerSource, String> {
    let asset_id = asset_id
        .trim()
        .parse::<AssetId>()
        .map_err(|_| format!("media id is not a valid identifier: {asset_id}"))?;
    Ok(LayerSource::Media {
        asset_id,
        playback: MediaPlayback::default(),
    })
}

/// Human-readable description of a layer's content source. `media_names`
/// resolves asset ids to filenames and `effect_names` resolves effect ids
/// to their registry display name; an id with no match falls back to the
/// bare kind ("Effect", "Media") — a raw UUID is never shown to the user
/// (Spec 65 §15.2). An effect outside the HTML catalog, such as a native
/// display face, has no resolvable name and reads simply as "Effect".
// Superseded in the live UI by the row's split title/kind rendering, but
// kept as the leptos-free pinned-contract function the §15.2 no-raw-UUID
// test exercises.
#[allow(dead_code)]
#[must_use]
pub fn layer_source_label(
    source: &LayerSource,
    media_names: &HashMap<String, String>,
    effect_names: &HashMap<String, String>,
) -> String {
    match source {
        LayerSource::Effect { effect_id, .. } => {
            let id = effect_id.to_string();
            effect_names
                .get(&id)
                .map(|name| format!("Effect {name}"))
                .unwrap_or_else(|| "Effect".to_owned())
        }
        LayerSource::Media { asset_id, .. } => {
            let id = asset_id.to_string();
            media_names
                .get(&id)
                .map(|name| format!("Media {name}"))
                .unwrap_or_else(|| "Media".to_owned())
        }
        LayerSource::ScreenRegion { .. } => "Screen region".to_owned(),
        LayerSource::WebViewport { url, .. } => format!("Web {url}"),
        LayerSource::ColorFill { .. } => "Color fill".to_owned(),
    }
}

/// Snake-case wire token for a blend mode.
#[must_use]
pub fn blend_value(mode: LayerBlendMode) -> &'static str {
    match mode {
        LayerBlendMode::Replace => "replace",
        LayerBlendMode::Alpha => "alpha",
        LayerBlendMode::Add => "add",
        LayerBlendMode::Screen => "screen",
        LayerBlendMode::Multiply => "multiply",
        LayerBlendMode::Overlay => "overlay",
        LayerBlendMode::SoftLight => "soft_light",
        LayerBlendMode::ColorDodge => "color_dodge",
        LayerBlendMode::Difference => "difference",
        LayerBlendMode::Tint => "tint",
        LayerBlendMode::LumaReveal => "luma_reveal",
    }
}

/// Parse a blend-mode token, defaulting to `Alpha` for an unknown value.
#[must_use]
pub fn parse_blend(value: &str) -> LayerBlendMode {
    match value {
        "replace" => LayerBlendMode::Replace,
        "add" => LayerBlendMode::Add,
        "screen" => LayerBlendMode::Screen,
        "multiply" => LayerBlendMode::Multiply,
        "overlay" => LayerBlendMode::Overlay,
        "soft_light" => LayerBlendMode::SoftLight,
        "color_dodge" => LayerBlendMode::ColorDodge,
        "difference" => LayerBlendMode::Difference,
        "tint" => LayerBlendMode::Tint,
        "luma_reveal" => LayerBlendMode::LumaReveal,
        _ => LayerBlendMode::Alpha,
    }
}

/// Blend-mode `(value, label)` options for the `SilkSelect` dropdown.
#[must_use]
pub fn blend_options() -> Vec<(String, String)> {
    [
        ("alpha", "Alpha"),
        ("replace", "Replace"),
        ("add", "Add"),
        ("screen", "Screen"),
        ("multiply", "Multiply"),
        ("overlay", "Overlay"),
        ("soft_light", "Soft Light"),
        ("color_dodge", "Color Dodge"),
        ("difference", "Difference"),
        ("tint", "Tint"),
        ("luma_reveal", "Luma Reveal"),
    ]
    .into_iter()
    .map(|(value, label)| (value.to_owned(), label.to_owned()))
    .collect()
}

/// Snake-case wire token for a fit mode.
#[must_use]
pub fn fit_value(mode: FitMode) -> &'static str {
    match mode {
        FitMode::Contain => "contain",
        FitMode::Cover => "cover",
        FitMode::Stretch => "stretch",
        FitMode::Tile => "tile",
        FitMode::Mirror => "mirror",
    }
}

/// Parse a fit-mode token, defaulting to `Cover` for an unknown value.
#[must_use]
pub fn parse_fit(value: &str) -> FitMode {
    match value {
        "contain" => FitMode::Contain,
        "stretch" => FitMode::Stretch,
        "tile" => FitMode::Tile,
        "mirror" => FitMode::Mirror,
        _ => FitMode::Cover,
    }
}

/// Fit-mode `(value, label)` options for the `SilkSelect` dropdown.
#[must_use]
pub fn fit_options() -> Vec<(String, String)> {
    [
        ("cover", "Cover"),
        ("contain", "Contain"),
        ("stretch", "Stretch"),
        ("tile", "Tile"),
        ("mirror", "Mirror"),
    ]
    .into_iter()
    .map(|(value, label)| (value.to_owned(), label.to_owned()))
    .collect()
}
