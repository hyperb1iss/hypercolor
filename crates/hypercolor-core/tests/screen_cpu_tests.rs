//! CPU screen capture contracts, grouped to share one integration-test link.

#[path = "screen_cpu/batch.rs"]
mod batch;
#[path = "screen_cpu/branch_processing.rs"]
mod branch_processing;
#[path = "screen_cpu/publication.rs"]
mod publication;
#[path = "screen_cpu/reducer.rs"]
mod reducer;
#[path = "screen_cpu/sampling_view.rs"]
mod sampling_view;
#[path = "screen_cpu/transformed_batch.rs"]
mod transformed_batch;
