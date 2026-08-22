use super::super::CompositionLayer;
use crate::render_thread::producer_queue::ProducerFrame;

mod bind_groups;
mod bypass;
mod execute;
mod layers;
mod sampling_state;

pub(crate) use bind_groups::PreparedProjectedComposeBindGroups;
#[cfg(feature = "allocation-contract-tests")]
pub(crate) use bind_groups::ProjectedLookupAllocationFixture;
#[cfg(test)]
pub(super) use bind_groups::{ComposeShaderMode, encode_compose_params};
pub(super) use bind_groups::{ComposeSourceBindGroupCache, create_compose_bind_group};
pub(super) use sampling_state::{SamplingReadbackBuffers, SamplingReadbackLatch};

fn screen_upload_content_keys(
    layers: &[CompositionLayer],
) -> impl Iterator<Item = super::ScreenUploadContentKey> + '_ {
    layers.iter().filter_map(|layer| {
        let ProducerFrame::ScreenPublication(publication) = &layer.frame else {
            return None;
        };
        let extent = publication.surface().extent();
        Some(super::ScreenUploadContentKey::new(
            publication.plan_generation(),
            publication.descriptor_identity(),
            publication.branch_sequence(),
            extent.width(),
            extent.height(),
        ))
    })
}

fn has_screen_upload_layers(layers: &[CompositionLayer]) -> bool {
    screen_upload_content_keys(layers).next().is_some()
}
