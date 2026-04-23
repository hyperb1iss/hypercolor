#![cfg(feature = "ws-core")]

use hypercolor_leptos_ext::MaybeSend;
use hypercolor_leptos_ext::ws::{
    BinaryFrameMetadata, BinaryFrameSchema, SchemaRange, negotiate_highest_common_schema,
};

#[derive(hypercolor_leptos_ext::ws::BinaryFrame)]
#[frame(tag = 0x03, schema = 2)]
struct CanvasFrameV2;

#[test]
fn derive_binary_frame_sets_schema_constants() {
    assert_eq!(CanvasFrameV2::TAG, 0x03);
    assert_eq!(CanvasFrameV2::SCHEMA, 2);
    assert_eq!(CanvasFrameV2::NAME, "CanvasFrameV2");
}

#[test]
fn derive_binary_frame_also_marks_metadata_trait() {
    fn assert_metadata<T: BinaryFrameMetadata>() {}

    assert_metadata::<CanvasFrameV2>();
}

#[test]
fn negotiate_schema_prefers_highest_common_version() {
    let client = SchemaRange::try_new(1, 4).expect("valid range");
    let server = SchemaRange::try_new(3, 6).expect("valid range");

    assert_eq!(negotiate_highest_common_schema(client, server), Some(4));
}

#[test]
fn negotiate_schema_returns_none_without_overlap() {
    let client = SchemaRange::try_new(1, 2).expect("valid range");
    let server = SchemaRange::try_new(3, 4).expect("valid range");

    assert_eq!(negotiate_highest_common_schema(client, server), None);
}

#[test]
fn schema_range_rejects_inverted_bounds() {
    assert_eq!(SchemaRange::try_new(9, 3), None);
}

#[test]
fn maybe_send_accepts_send_types() {
    fn assert_maybe_send<T: MaybeSend>() {}

    assert_maybe_send::<u32>();
}
