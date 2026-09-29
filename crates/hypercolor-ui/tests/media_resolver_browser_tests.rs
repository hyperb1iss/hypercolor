//! The media resolver end to end in a browser: chunked bytes become a typed
//! `Blob` whose `blob:` URL reads back the same bytes.
#![cfg(target_arch = "wasm32")]

#[path = "support/chunking_transport.rs"]
mod chunking_transport;

use std::future::Future;
use std::rc::Rc;

use chunking_transport::{ChunkingTransport, Reply, SVG, webp};
use hypercolor_ui::media::{
    CACHE_MAX_BYTES, CACHE_MAX_ENTRIES, MediaError, MediaResolver, MediaUse,
};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const COVER: &str = "/api/v1/effects/prism/cover";
const COVER_BYTES: usize = 134 * 1024;

#[wasm_bindgen(inline_js = r#"
export async function readBlobUrl(url) {
  const response = await fetch(url);
  return new Uint8Array(await response.arrayBuffer());
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = readBlobUrl)]
    fn read_blob_url(url: &str) -> js_sys::Promise;
}

fn resolver(transport: ChunkingTransport) -> (MediaResolver, Rc<ChunkingTransport>) {
    let transport = Rc::new(transport);
    (MediaResolver::new(transport.clone()), transport)
}

#[wasm_bindgen_test]
async fn a_134_kb_cover_arrives_through_chunks_as_a_blob_url() {
    let cover = webp(COVER_BYTES);
    let (resolver, transport) = resolver(
        ChunkingTransport::new(24 * 1024).reply(COVER, Reply::ok("image/webp", cover.clone())),
    );
    let blob = resolver
        .resolve(COVER, MediaUse::Artwork)
        .await
        .expect("cover resolves");
    assert_eq!(blob.type_(), "image/webp");
    assert_eq!(blob.size(), COVER_BYTES as f64);
    assert!(transport.chunk_sizes.borrow().len() >= 6);

    let url = web_sys::Url::create_object_url_with_blob(&blob).expect("blob url");
    assert!(url.starts_with("blob:"));
    let bytes = JsFuture::from(read_blob_url(&url))
        .await
        .expect("blob url reads");
    assert_eq!(js_sys::Uint8Array::new(&bytes).to_vec(), cover);
    web_sys::Url::revoke_object_url(&url).expect("revoke");

    resolver
        .resolve(COVER, MediaUse::Artwork)
        .await
        .expect("cached cover");
    assert_eq!(transport.requests.borrow().len(), 1, "artwork is cached");
}

#[wasm_bindgen_test]
async fn an_svg_is_refused_and_live_stills_are_never_cached() {
    let still = "/api/v1/displays/lcd/frame?ts=1";
    let (resolver, transport) = resolver(
        ChunkingTransport::new(1024)
            .reply(COVER, Reply::ok("image/svg+xml", SVG.to_vec()))
            .reply(
                still,
                Reply::ok("image/jpeg", vec![0xff, 0xd8, 0xff, 0xe0, 0, 0]),
            ),
    );
    let refused = resolver
        .resolve(COVER, MediaUse::Artwork)
        .await
        .expect_err("svg is refused");
    assert_eq!(
        refused,
        MediaError::MediaType(Some("image/svg+xml".to_owned()))
    );
    for _ in 0..2 {
        let blob = resolver
            .resolve(still, MediaUse::LiveStill)
            .await
            .expect("still resolves");
        assert_eq!(blob.type_(), "image/jpeg");
    }
    assert_eq!(
        transport.requests.borrow().as_slice(),
        [COVER, still, still]
    );
}

#[wasm_bindgen(inline_js = r#"
export function sleep(ms) { return new Promise((resolve) => setTimeout(resolve, ms)); }
"#)]
extern "C" {
    fn sleep(ms: u32) -> js_sys::Promise;
}

#[wasm_bindgen_test]
async fn sites_asking_for_one_route_in_flight_share_a_fetch() {
    let gate = Rc::new(chunking_transport::Gate::default());
    let (resolver, transport) = resolver(
        ChunkingTransport::new(8 * 1024)
            .reply(COVER, Reply::ok("image/webp", webp(COVER_BYTES)))
            .gated(Rc::clone(&gate)),
    );
    let opener = Rc::clone(&gate);
    wasm_bindgen_futures::spawn_local(async move {
        JsFuture::from(sleep(20)).await.expect("timer");
        opener.open();
    });
    let (first, second) = futures_util::future::join(
        resolver.resolve(COVER, MediaUse::Artwork),
        resolver.resolve(COVER, MediaUse::Artwork),
    )
    .await;
    let (first, second) = (first.expect("first site"), second.expect("second site"));
    assert!(
        js_sys::Object::is(&first, &second),
        "one Blob for both sites"
    );
    assert_eq!(transport.requests.borrow().as_slice(), [COVER]);
}

#[wasm_bindgen_test]
async fn a_fetch_every_site_abandoned_is_cancelled() {
    let still = "/api/v1/displays/lcd/frame?ts=9";
    let gate = Rc::new(chunking_transport::Gate::default());
    let transport = Rc::new(
        ChunkingTransport::new(1024)
            .reply(
                still,
                Reply::ok("image/jpeg", vec![0xff, 0xd8, 0xff, 0xe0, 0, 0]),
            )
            .gated(Rc::clone(&gate)),
    );
    // One permit, so a permit the abandoned fetch kept would hang the next.
    let resolver =
        MediaResolver::with_limits(transport.clone(), 1, CACHE_MAX_ENTRIES, CACHE_MAX_BYTES);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    {
        let mut first = Box::pin(resolver.resolve(still, MediaUse::LiveStill));
        let mut second = Box::pin(resolver.resolve(still, MediaUse::LiveStill));
        assert!(first.as_mut().poll(&mut context).is_pending());
        assert!(second.as_mut().poll(&mut context).is_pending());
        assert_eq!(
            transport.exchanges.borrow().len(),
            1,
            "one exchange for both"
        );
        drop(first);
        assert!(
            !transport.exchanges.borrow()[0].is_cancelled(),
            "a remaining site keeps the fetch"
        );
    }
    assert!(
        transport.exchanges.borrow()[0].is_cancelled(),
        "the last site leaving cancels the exchange"
    );

    gate.open();
    let blob = resolver
        .resolve(still, MediaUse::LiveStill)
        .await
        .expect("a fresh fetch after the abandoned one");
    assert_eq!(blob.type_(), "image/jpeg");
    assert_eq!(transport.requests.borrow().len(), 2);
}
