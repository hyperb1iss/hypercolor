use std::alloc::System;
use std::time::Duration;

use hypercolor_core::input::media::{ArtworkFetcher, ArtworkPolicy};
use image::ImageEncoder as _;
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

fn artwork_policy(max_source_bytes: usize) -> ArtworkPolicy {
    ArtworkPolicy {
        fetch_timeout: Duration::from_millis(250),
        max_source_bytes,
        max_source_dimension: 1_024,
        max_source_pixels: 1_048_576,
        max_decode_bytes: 4 * 1_048_576,
        max_output_dimension: 64,
        max_data_url_bytes: 128 * 1_024,
        max_redirects: 1,
    }
}

fn png_with_icc_profile(profile_bytes: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
    encoder
        .set_icc_profile(vec![0; profile_bytes])
        .expect("test ICC profile is accepted");
    encoder
        .write_image(&[0, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .expect("test PNG encodes");
    bytes
}

#[test]
fn compressed_png_metadata_obeys_the_decode_allocation_limit() {
    const CHILD_ENV: &str = "HYPERCOLOR_MEDIA_ICC_ALLOCATION_CHILD";
    if std::env::var_os(CHILD_ENV).is_none() {
        let status = std::process::Command::new(std::env::current_exe().expect("test path exists"))
            .args([
                "--exact",
                "compressed_png_metadata_obeys_the_decode_allocation_limit",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .status()
            .expect("allocation probe child starts");
        assert!(status.success(), "allocation probe child failed");
        return;
    }

    let bytes = png_with_icc_profile(64 * 1024 * 1024);
    assert!(
        bytes.len() < 256 * 1024,
        "fixture must remain highly compressed"
    );
    let mut policy = artwork_policy(bytes.len());
    policy.max_decode_bytes = 512 * 1024;
    let fetcher = ArtworkFetcher::new(policy).expect("bounded policy builds");
    let mut region = Region::new(GLOBAL);
    region.reset();

    let result = fetcher.encode_data_url(&bytes);
    let allocated = region.change().bytes_allocated;

    assert!(
        result.is_ok(),
        "oversized optional metadata is ignored safely"
    );
    assert!(
        allocated < 16 * 1024 * 1024,
        "dimension probing allocated {allocated} bytes for bounded ancillary metadata"
    );
}
