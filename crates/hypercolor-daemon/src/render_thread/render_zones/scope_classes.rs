//! A zone's outputs grouped by the layers that apply to them.
//!
//! A layer with a scope applies to some of its zone's outputs. Outputs that
//! see exactly the same layers form a scope class, and each class composes
//! into a texture of its own from the layer frames the zone renders once.
//! An output samples its class's texture and no other, so a scoped layer
//! never reaches an output outside its scope, however the outputs' boxes
//! overlap on the canvas and whatever footprint their sampling reads.
//!
//! A zone with no scoped layer has one class and keeps no record here: it
//! renders exactly as a zone always has.

use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_core::spatial::{LayerFraming, is_led_sampled_zone};
use hypercolor_types::layer::LayerScope;
use hypercolor_types::scene::{Zone, ZoneId};
use hypercolor_types::spatial::SpatialLayout;

/// Index into [`ZoneScopeClasses::classes`]. Built with a checked
/// conversion; a zone whose classes do not fit is refused at reconcile like
/// any other resource it cannot admit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ScopeClassIndex(u32);

impl ScopeClassIndex {
    /// The class of the zone's unscoped layers.
    pub(crate) const BASE: Self = Self(0);

    /// Not a class: the key a zone's picture is kept under, the one frame
    /// that shows every output's own class for a preview.
    const PICTURE: Self = Self(u32::MAX);

    fn new(index: usize) -> Result<Self, ScopeClassOverflow> {
        u32::try_from(index)
            .ok()
            .filter(|index| *index < Self::PICTURE.0)
            .map(Self)
            .ok_or(ScopeClassOverflow)
    }

    pub(crate) const fn is_base(self) -> bool {
        self.0 == 0
    }

    fn position(self) -> usize {
        usize::try_from(self.0).unwrap_or(usize::MAX)
    }
}

impl std::fmt::Display for ScopeClassIndex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A zone has more scope classes than an index holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("zone has more scope classes than the render thread can index")]
pub(crate) struct ScopeClassOverflow;

/// One sampling source of a scene: a scope class of a zone. Everything a
/// scene zone keeps per composed frame is kept under this key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ZoneClassKey {
    pub(crate) zone_id: ZoneId,
    pub(crate) class: ScopeClassIndex,
}

impl ZoneClassKey {
    pub(crate) const fn new(zone_id: ZoneId, class: ScopeClassIndex) -> Self {
        Self { zone_id, class }
    }

    /// The zone's base class: the whole of a zone with no scoped layer.
    pub(crate) const fn base(zone_id: ZoneId) -> Self {
        Self::new(zone_id, ScopeClassIndex::BASE)
    }

    /// The key of the picture of a zone with several classes: its base
    /// class with every other output's box drawn from that output's class.
    /// It is made for previews and no LED samples it.
    pub(crate) const fn picture(zone_id: ZoneId) -> Self {
        Self::new(zone_id, ScopeClassIndex::PICTURE)
    }
}

impl From<ZoneId> for ZoneClassKey {
    fn from(zone_id: ZoneId) -> Self {
        Self::base(zone_id)
    }
}

/// One zone's outputs grouped by the layers that apply to them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ZoneScopeClasses {
    /// `classes[0]` is the base class. The others follow in the order of
    /// the first output that uses them.
    classes: Vec<ScopeClass>,
    /// Class per output, in `zone.layout.zones` order.
    class_of_output: Vec<ScopeClassIndex>,
    /// How each of the zone's layers is framed, in `zone.layers` order.
    framings: Vec<LayerFraming>,
    /// Each layer's scope, shared with the touch mapping.
    scopes: Vec<Option<Arc<LayerScope>>>,
}

/// The outputs of a zone that see one stack of layers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScopeClass {
    /// Indices into `zone.layers`, bottom to top.
    pub(crate) layers: Vec<usize>,
    /// Indices into `zone.layout.zones`.
    pub(crate) outputs: Vec<usize>,
    /// Longest run of layers shared with the base class from the bottom.
    pub(crate) shared_base_prefix: usize,
}

impl ZoneScopeClasses {
    /// The classes of `zone`, or `None` for a zone with no enabled scoped
    /// layer: one class, the whole zone, rendered the way a zone always
    /// was.
    ///
    /// A zone that has one still gets a record when every scoped layer is
    /// dormant. It then has the base class alone, and the record says which
    /// layers render nothing.
    pub(crate) fn of(zone: &Zone) -> Result<Option<Self>, ScopeClassOverflow> {
        if !zone.enabled
            || !zone
                .layers
                .iter()
                .any(|layer| layer.enabled && layer.scope.is_some())
        {
            return Ok(None);
        }
        let framings = zone
            .layers
            .iter()
            .map(|layer| LayerFraming::of(zone, layer))
            .collect::<Vec<_>>();
        let scopes = zone
            .layers
            .iter()
            .map(|layer| layer.scope.clone().map(Arc::new))
            .collect::<Vec<_>>();
        let renders = |index: usize| zone.layers[index].enabled && !framings[index].is_dormant();
        let base_layers = (0..zone.layers.len())
            .filter(|&index| renders(index) && zone.layers[index].scope.is_none())
            .collect::<Vec<_>>();
        let mut classes = vec![ScopeClass {
            shared_base_prefix: base_layers.len(),
            layers: base_layers,
            outputs: Vec::new(),
        }];
        let mut class_of_output = Vec::with_capacity(zone.layout.zones.len());
        for (output_index, output) in zone.layout.zones.iter().enumerate() {
            let stack = (0..zone.layers.len())
                .filter(|&index| {
                    renders(index)
                        && zone.layers[index]
                            .scope
                            .as_ref()
                            .is_none_or(|scope| scope.contains(&output.id))
                })
                .collect::<Vec<_>>();
            let position = classes
                .iter()
                .position(|class| class.layers == stack)
                .unwrap_or_else(|| {
                    let shared_base_prefix = stack
                        .iter()
                        .zip(&classes[0].layers)
                        .take_while(|(layer, base)| layer == base)
                        .count();
                    classes.push(ScopeClass {
                        layers: stack,
                        outputs: Vec::new(),
                        shared_base_prefix,
                    });
                    classes.len() - 1
                });
            classes[position].outputs.push(output_index);
            class_of_output.push(ScopeClassIndex::new(position)?);
        }
        Ok(Some(Self {
            classes,
            class_of_output,
            framings,
            scopes,
        }))
    }

    pub(crate) fn class_count(&self) -> usize {
        self.classes.len()
    }

    /// Whether the zone composes one stack: every scoped layer it has is
    /// dormant.
    pub(crate) fn is_single(&self) -> bool {
        self.classes.len() == 1
    }

    /// Every class with its index, base class first.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (ScopeClassIndex, &ScopeClass)> {
        self.classes
            .iter()
            .enumerate()
            .map(|(position, class)| (ScopeClassIndex(index_u32(position)), class))
    }

    pub(crate) fn base(&self) -> &ScopeClass {
        &self.classes[0]
    }

    /// The class of the output at `output_index` in the zone's layout, or
    /// the base class for an index the layout does not hold.
    pub(crate) fn class_of_output(&self, output_index: usize) -> ScopeClassIndex {
        self.class_of_output
            .get(output_index)
            .copied()
            .unwrap_or(ScopeClassIndex::BASE)
    }

    /// How the layer at `layer_index` in the zone's stack is framed.
    pub(crate) fn framing(&self, layer_index: usize) -> LayerFraming {
        self.framings
            .get(layer_index)
            .copied()
            .unwrap_or(LayerFraming::Zone)
    }

    /// The scope of the layer at `layer_index`, shared for the touch
    /// mapping.
    pub(crate) fn scope(&self, layer_index: usize) -> Option<Arc<LayerScope>> {
        self.scopes.get(layer_index).cloned().flatten()
    }

    /// Whether a class adds its layers on top of the whole base stack, so
    /// it can start from the base class's composed pixels.
    pub(crate) fn extends_base(&self, class: &ScopeClass) -> bool {
        class.shared_base_prefix == self.classes[0].layers.len()
    }

    /// The zone's layout with only the LED outputs of one class, for that
    /// class's sampling plan.
    pub(crate) fn class_layout(
        &self,
        layout: &SpatialLayout,
        class: ScopeClassIndex,
    ) -> SpatialLayout {
        let outputs = self
            .classes
            .get(class.position())
            .map(|class| class.outputs.as_slice())
            .unwrap_or_default();
        SpatialLayout {
            zones: outputs
                .iter()
                .filter_map(|&index| layout.zones.get(index))
                .filter(|output| is_led_sampled_zone(output))
                .cloned()
                .collect(),
            ..layout.clone()
        }
    }

    /// Whether a class has an LED to sample.
    pub(crate) fn class_samples_leds(&self, layout: &SpatialLayout, class: &ScopeClass) -> bool {
        class
            .outputs
            .iter()
            .filter_map(|&index| layout.zones.get(index))
            .any(is_led_sampled_zone)
    }
}

fn index_u32(position: usize) -> u32 {
    u32::try_from(position).unwrap_or(u32::MAX)
}

/// The scope classes a zone composes, base class first: the base class
/// alone for a zone with no scoped layer, or with none that is live.
pub(crate) fn zone_class_keys(
    scope_classes: &HashMap<ZoneId, Arc<ZoneScopeClasses>>,
    zone_id: ZoneId,
) -> impl Iterator<Item = ZoneClassKey> + '_ {
    let classes = multi_class(scope_classes, zone_id);
    let base = classes
        .is_none()
        .then(|| ZoneClassKey::base(zone_id))
        .into_iter();
    base.chain(
        classes
            .into_iter()
            .flat_map(|classes| classes.iter())
            .map(move |(class, _)| ZoneClassKey::new(zone_id, class)),
    )
}

/// The classes of a zone that composes more than one stack.
pub(crate) fn multi_class(
    scope_classes: &HashMap<ZoneId, Arc<ZoneScopeClasses>>,
    zone_id: ZoneId,
) -> Option<&Arc<ZoneScopeClasses>> {
    scope_classes
        .get(&zone_id)
        .filter(|classes| !classes.is_single())
}

#[cfg(test)]
mod tests {
    use hypercolor_types::layer::{LayerScope, LayerSource, SceneLayer, SceneLayerId};

    use super::super::tests::{point_zone_at, sample_zone};
    use super::*;

    fn layer(scope: Option<&[&str]>) -> SceneLayer {
        SceneLayer {
            id: SceneLayerId::new(),
            name: None,
            source: LayerSource::ColorFill {
                rgba: [1.0, 0.0, 0.0, 1.0],
            },
            blend: hypercolor_types::layer::BlendMode::Alpha,
            opacity: 1.0,
            transform: hypercolor_types::layer::LayerTransform::default(),
            adjust: hypercolor_types::layer::LayerAdjust::default(),
            bindings: Vec::new(),
            enabled: true,
            scope: scope.map(|ids| LayerScope {
                outputs: ids.iter().map(|id| (*id).to_owned()).collect(),
            }),
        }
    }

    fn zone(layers: Vec<SceneLayer>, outputs: &[&str]) -> Zone {
        let mut zone = sample_zone(64, 48);
        zone.layers = layers;
        zone.layout.zones = outputs
            .iter()
            .map(|id| point_zone_at(id, 0.5, 0.5))
            .collect();
        zone
    }

    fn classes(zone: &Zone) -> ZoneScopeClasses {
        ZoneScopeClasses::of(zone)
            .expect("classes fit")
            .expect("zone has a scoped layer")
    }

    #[test]
    fn a_zone_with_no_scoped_layer_keeps_no_classes() {
        let unscoped = zone(vec![layer(None), layer(None)], &["a", "b"]);
        assert_eq!(ZoneScopeClasses::of(&unscoped), Ok(None));

        let mut disabled = zone(vec![layer(None), layer(Some(&["a"]))], &["a", "b"]);
        disabled.layers[1].enabled = false;
        assert_eq!(ZoneScopeClasses::of(&disabled), Ok(None));
    }

    #[test]
    fn outputs_group_by_the_layers_that_apply_to_them() {
        // base, keys on a and c, base, glow on c.
        let zone = zone(
            vec![
                layer(None),
                layer(Some(&["a", "c"])),
                layer(None),
                layer(Some(&["c"])),
            ],
            &["a", "b", "c", "d"],
        );
        let classes = classes(&zone);

        assert_eq!(classes.class_count(), 3);
        assert_eq!(classes.base().layers, vec![0, 2]);
        assert_eq!(classes.base().outputs, vec![1, 3]);
        let stacks = classes
            .iter()
            .map(|(_, class)| (class.layers.clone(), class.outputs.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            stacks,
            vec![
                (vec![0, 2], vec![1, 3]),
                (vec![0, 1, 2], vec![0]),
                (vec![0, 1, 2, 3], vec![2]),
            ]
        );
        // A scoped layer under an unscoped one shares only the layers
        // beneath it with the base class.
        assert!(
            classes
                .iter()
                .skip(1)
                .all(|(_, class)| class.shared_base_prefix == 1)
        );
        assert!(
            classes
                .iter()
                .skip(1)
                .all(|(_, class)| !classes.extends_base(class))
        );
        assert_eq!(
            (0..4)
                .map(|output| classes.class_of_output(output))
                .collect::<Vec<_>>(),
            vec![
                ScopeClassIndex(1),
                ScopeClassIndex::BASE,
                ScopeClassIndex(2),
                ScopeClassIndex::BASE
            ]
        );
    }

    #[test]
    fn an_overlay_on_top_extends_the_base_stack() {
        let zone = zone(vec![layer(None), layer(Some(&["b"]))], &["a", "b"]);
        let classes = classes(&zone);

        let (_, overlay) = classes.iter().nth(1).expect("the overlay's class");
        assert_eq!(overlay.layers, vec![0, 1]);
        assert_eq!(overlay.shared_base_prefix, 1);
        assert!(classes.extends_base(overlay));
    }

    #[test]
    fn the_base_class_exists_when_no_output_uses_it() {
        let zone = zone(vec![layer(None), layer(Some(&["a", "b"]))], &["a", "b"]);
        let classes = classes(&zone);

        assert_eq!(classes.class_count(), 2);
        assert!(classes.base().outputs.is_empty());
        assert_eq!(classes.base().layers, vec![0]);
    }

    #[test]
    fn a_scoped_bottom_layer_leaves_other_outputs_an_empty_stack() {
        let zone = zone(vec![layer(Some(&["a"]))], &["a", "b"]);
        let classes = classes(&zone);

        assert!(classes.base().layers.is_empty());
        assert_eq!(classes.base().outputs, vec![1]);
        let (_, scoped) = classes.iter().nth(1).expect("the scoped class");
        assert_eq!(scoped.layers, vec![0]);
        assert!(classes.extends_base(scoped));
    }

    #[test]
    fn a_dormant_layer_is_in_no_class_and_is_never_widened() {
        let zone = zone(vec![layer(None), layer(Some(&["gone"]))], &["a", "b"]);
        let classes = classes(&zone);

        assert!(classes.is_single());
        assert_eq!(classes.base().layers, vec![0]);
        assert_eq!(classes.base().outputs, vec![0, 1]);
        assert!(classes.framing(1).is_dormant());
        assert_eq!(classes.framing(0), LayerFraming::Zone);
    }
}
