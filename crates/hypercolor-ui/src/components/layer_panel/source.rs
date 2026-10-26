//! Leptos-free vocabulary for the layer panel: blend/fit option lists,
//! source labels, and the Add-layer picker model.
//!
//! Deliberately free of `leptos` and `crate::` paths so the layer-panel
//! contract is exercisable from `tests/layer_panel_tests.rs` via a
//! `#[path]` include, mirroring `route_ui.rs` and `label_utils.rs`.

use std::collections::HashMap;

use hypercolor_types::api::scene::CreateLayerRequest;
use hypercolor_types::asset::AssetId;
use hypercolor_types::effect::{EffectCategory, EffectId};
use hypercolor_types::layer::{BlendMode, LayerAdjust, LayerSource, LayerTransform, MediaPlayback};
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::ZoneRole;

use crate::api::ZoneResource;
use crate::preferences::EffectPreferences;
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
}

impl EffectPickerMode {
    #[must_use]
    pub const fn tab_label(self) -> &'static str {
        match self {
            Self::Faces => "Face",
            Self::Effects => "Effect",
        }
    }

    #[must_use]
    pub const fn search_placeholder(self) -> &'static str {
        match self {
            Self::Faces => "Search faces and effects...",
            Self::Effects => "Search effects...",
        }
    }

    #[must_use]
    pub const fn empty_detail(self) -> &'static str {
        match self {
            Self::Faces => "No matching faces or effects",
            Self::Effects => "No matching effects",
        }
    }

    #[must_use]
    pub const fn includes_category(self, category: EffectCategory) -> bool {
        let is_display = matches!(category, EffectCategory::Display);
        match self {
            Self::Effects => !is_display,
            Self::Faces => true,
        }
    }

    #[must_use]
    pub const fn sort_bucket(self, category: EffectCategory) -> u8 {
        if matches!(self, Self::Faces) && matches!(category, EffectCategory::Display) {
            0
        } else {
            1
        }
    }
}

/// Where an Add-layer action sends the new layer. A layer lands on the
/// active tab. Screens share one tab, so on a screen the picker can also
/// send the layer to every screen at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddLayerScope {
    /// The selected surface only — the default.
    ThisSurface,
    /// Every display-face screen.
    AllScreens,
}

impl AddLayerScope {
    /// User-facing label for the scope selector, which only a screen shows.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::ThisSurface => "This screen",
            Self::AllScreens => "All screens",
        }
    }
}

#[must_use]
pub fn effect_picker_mode(
    scope: AddLayerScope,
    selected_role: Option<ZoneRole>,
) -> EffectPickerMode {
    if scope == AddLayerScope::AllScreens || selected_role == Some(ZoneRole::Display) {
        EffectPickerMode::Faces
    } else {
        EffectPickerMode::Effects
    }
}

#[must_use]
pub fn effect_category_label(category: EffectCategory) -> String {
    if category == EffectCategory::Display {
        "face".to_owned()
    } else {
        category.as_str().to_owned()
    }
}

#[must_use]
pub fn effect_picker_matches_query(name: &str, category: EffectCategory, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || name.to_lowercase().contains(&query)
        || category.as_str().contains(&query)
        || effect_category_label(category)
            .to_lowercase()
            .contains(&query)
}

/// The scopes worth offering for the selected surface. An LED zone adds
/// to itself alone; a screen can also add to every screen when the scene
/// has more than one. A result shorter than two means "show no selector".
#[must_use]
pub fn available_add_layer_scopes(
    zones: &[ZoneResource],
    selected_role: Option<ZoneRole>,
) -> Vec<AddLayerScope> {
    let screens = zones
        .iter()
        .filter(|zone| zone.role == ZoneRole::Display)
        .count();
    if selected_role == Some(ZoneRole::Display) && screens > 1 {
        vec![AddLayerScope::ThisSurface, AddLayerScope::AllScreens]
    } else {
        Vec::new()
    }
}

/// Resolve a scope to the render-zone ids that should receive the layer,
/// in scene order. Targets are deduplicated so a scope can never queue the
/// same surface twice.
#[must_use]
pub fn resolve_add_layer_targets(
    scope: AddLayerScope,
    zones: &[ZoneResource],
    selected_zone_id: &str,
) -> Vec<String> {
    match scope {
        AddLayerScope::ThisSurface => vec![selected_zone_id.to_owned()],
        AddLayerScope::AllScreens => zones
            .iter()
            .filter(|zone| zone.role == ZoneRole::Display)
            .map(|zone| zone.id.to_string())
            .collect(),
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

/// Build an effect layer source that starts in the user's remembered state
/// for that effect: its saved preset and control values, when there are
/// any, so that state rides in the one request that creates the layer
/// instead of being patched on afterwards. A saved preset id that no
/// longer parses is dropped; the saved controls still apply.
pub fn remembered_effect_layer_source(
    effect_id: &str,
    prefs: Option<&EffectPreferences>,
) -> Result<LayerSource, String> {
    let mut source = effect_layer_source(effect_id)?;
    if let Some(prefs) = prefs
        && let LayerSource::Effect {
            controls,
            preset_id,
            ..
        } = &mut source
    {
        controls.clone_from(&prefs.control_values);
        *preset_id = prefs
            .preset_id
            .as_deref()
            .and_then(|raw| Uuid::parse_str(raw.trim()).ok())
            .map(PresetId);
    }
    Ok(source)
}

/// The request the layer panel sends to add one picked layer to a surface.
#[must_use]
pub fn add_layer_request(
    name: Option<String>,
    source: LayerSource,
    existing_layer_count: usize,
) -> CreateLayerRequest {
    let blend = default_blend_for_added_layer(&source, existing_layer_count);
    CreateLayerRequest {
        name,
        source,
        blend: Some(blend),
        opacity: Some(1.0),
        transform: Some(LayerTransform::default()),
        adjust: Some(LayerAdjust::default()),
        bindings: None,
        enabled: None,
    }
}

/// Build a media layer source from an asset id string.
pub fn media_layer_source(asset_id: &str) -> Result<LayerSource, String> {
    let asset_id = asset_id
        .trim()
        .parse::<AssetId>()
        .map_err(|_| format!("media id is not a valid identifier: {asset_id}"))?;
    Ok(media_layer_source_for(asset_id))
}

/// Build a media layer source from an asset id the caller already holds
/// typed — the library grid path, where parsing cannot fail.
#[must_use]
pub fn media_layer_source_for(asset_id: AssetId) -> LayerSource {
    LayerSource::Media {
        asset_id,
        playback: MediaPlayback::default(),
    }
}

/// Initial blend mode for a layer added through the picker.
#[must_use]
pub fn default_blend_for_added_layer(
    source: &LayerSource,
    existing_layer_count: usize,
) -> BlendMode {
    if existing_layer_count > 0 && matches!(source, LayerSource::Effect { .. }) {
        BlendMode::Screen
    } else {
        BlendMode::Alpha
    }
}

/// Snake-case wire token for a blend mode.
#[must_use]
pub fn blend_value(mode: BlendMode) -> &'static str {
    match mode {
        BlendMode::Replace => "replace",
        BlendMode::Alpha => "alpha",
        BlendMode::Add => "add",
        BlendMode::Screen => "screen",
        BlendMode::Multiply => "multiply",
        BlendMode::Overlay => "overlay",
        BlendMode::SoftLight => "soft_light",
        BlendMode::ColorDodge => "color_dodge",
        BlendMode::Difference => "difference",
        BlendMode::Tint => "tint",
        BlendMode::LumaReveal => "luma_reveal",
    }
}

/// Parse a blend-mode token, defaulting to `Alpha` for an unknown value.
#[must_use]
pub fn parse_blend(value: &str) -> BlendMode {
    match value {
        "replace" => BlendMode::Replace,
        "add" => BlendMode::Add,
        "screen" => BlendMode::Screen,
        "multiply" => BlendMode::Multiply,
        "overlay" => BlendMode::Overlay,
        "soft_light" => BlendMode::SoftLight,
        "color_dodge" => BlendMode::ColorDodge,
        "difference" => BlendMode::Difference,
        "tint" => BlendMode::Tint,
        "luma_reveal" => BlendMode::LumaReveal,
        _ => BlendMode::Alpha,
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
