//! The media resolver's checks, byte handling, LRU and concurrency cap.

#[path = "support/chunking_transport.rs"]
mod chunking_transport;

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use chunking_transport::{ChunkingTransport, Reply, SVG, webp};
use hypercolor_ui::media::{
    CACHE_MAX_ENTRY_BYTES, MAX_IMAGE_BYTES, MediaClass, MediaError, MediaLimiter, MediaLru,
    MediaUse, allowed_media_type, fetch_media, is_daemon_media_route, signature_matches,
};

const COVER: &str = "/api/v1/effects/prism/cover";
/// The largest bundled cover, several times what one sealed frame carries.
const COVER_BYTES: usize = 134 * 1024;

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    for _ in 0..64 {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
    panic!("the test transport never settled");
}

fn fetch(
    transport: &ChunkingTransport,
    route: &str,
    class: MediaClass,
) -> (
    Result<hypercolor_ui::media::FetchedMedia, MediaError>,
    Vec<u8>,
) {
    let mut collected = Vec::new();
    let result = block_on(fetch_media(transport, route, class, &mut |chunk| {
        collected.extend_from_slice(&chunk);
    }));
    (result, collected)
}

#[test]
fn a_cover_larger_than_one_frame_arrives_whole_through_chunks() {
    let cover = webp(COVER_BYTES);
    for chunk in [24 * 1024, 5_000, 7, 1] {
        let transport =
            ChunkingTransport::new(chunk).reply(COVER, Reply::ok("image/webp", cover.clone()));
        let (result, collected) = fetch(&transport, COVER, MediaClass::Image);
        let fetched = result.expect("cover resolves");
        assert_eq!(fetched.media_type, "image/webp", "chunk {chunk}");
        assert_eq!(fetched.len, COVER_BYTES as u64, "chunk {chunk}");
        assert_eq!(collected, cover, "chunk {chunk}");
        assert_eq!(
            transport.chunk_sizes.borrow().iter().sum::<usize>(),
            COVER_BYTES
        );
        assert!(transport.chunk_sizes.borrow().len() > 1, "chunk {chunk}");
        assert_eq!(transport.requests.borrow().as_slice(), [COVER]);
    }
}

#[test]
fn an_svg_is_refused_before_any_byte_reaches_the_sink() {
    let transport =
        ChunkingTransport::new(64).reply(COVER, Reply::ok("image/svg+xml", SVG.to_vec()));
    let (result, collected) = fetch(&transport, COVER, MediaClass::Image);
    assert_eq!(
        result,
        Err(MediaError::MediaType(Some("image/svg+xml".to_owned())))
    );
    assert!(collected.is_empty());
}

#[test]
fn a_body_that_is_not_what_it_claims_is_refused() {
    for (content_type, body) in [
        ("image/webp", SVG.to_vec()),
        ("image/png", webp(4_096)),
        ("image/jpeg", b"<html><body>hi</body></html>".to_vec()),
        ("image/webp", Vec::new()),
        ("image/webp", b"RIFF".to_vec()),
    ] {
        let transport = ChunkingTransport::new(3).reply(COVER, Reply::ok(content_type, body));
        let (result, collected) = fetch(&transport, COVER, MediaClass::Image);
        assert_eq!(result, Err(MediaError::Signature), "{content_type}");
        assert!(collected.is_empty(), "{content_type}");
    }
}

#[test]
fn only_raster_and_video_types_pass_the_allowlist() {
    for (content_type, expected) in [
        ("image/webp", Some("image/webp")),
        ("IMAGE/WebP; charset=binary", Some("image/webp")),
        (" image/png ", Some("image/png")),
        ("image/apng", Some("image/apng")),
        ("image/jpeg", Some("image/jpeg")),
        ("image/gif", Some("image/gif")),
        ("image/svg+xml", None),
        ("image/x-icon", None),
        ("text/html", None),
        ("application/octet-stream", None),
        ("video/mp4", None),
        ("", None),
    ] {
        assert_eq!(
            allowed_media_type(MediaClass::Image, content_type),
            expected,
            "{content_type}"
        );
    }
    assert_eq!(
        allowed_media_type(MediaClass::Video, "video/mp4"),
        Some("video/mp4")
    );
    assert_eq!(
        allowed_media_type(MediaClass::Video, "video/webm; codecs=vp8"),
        Some("video/webm")
    );
    assert_eq!(allowed_media_type(MediaClass::Video, "image/webp"), None);
    assert_eq!(allowed_media_type(MediaClass::Video, "video/ogg"), None);
}

#[test]
fn signatures_match_the_daemons_upload_sniffing() {
    assert!(signature_matches("image/webp", &webp(64)));
    assert!(signature_matches("image/png", b"\x89PNG\r\n\x1a\n...."));
    assert!(signature_matches("image/apng", b"\x89PNG\r\n\x1a\n...."));
    assert!(signature_matches("image/jpeg", &[0xff, 0xd8, 0xff, 0xe0]));
    assert!(signature_matches("image/gif", b"GIF89a...."));
    assert!(signature_matches("image/gif", b"GIF87a...."));
    assert!(signature_matches("video/mp4", b"\0\0\0\x18ftypisom"));
    assert!(signature_matches(
        "video/webm",
        &[0x1a, 0x45, 0xdf, 0xa3, 0x01]
    ));
    assert!(!signature_matches("image/webp", b"RIFF\0\0\0\0WAVE"));
    assert!(!signature_matches("video/mp4", b"\0\0\0\x18moov"));
    assert!(!signature_matches("image/svg+xml", SVG));
}

#[test]
fn statuses_and_missing_types_are_refused() {
    let transport = ChunkingTransport::new(64).reply(
        "/api/v1/assets/1/thumbnail",
        Reply {
            status: 200,
            content_type: None,
            prefix: webp(64).into(),
            filler: 0,
        },
    );
    let (result, _) = fetch(&transport, COVER, MediaClass::Image);
    assert_eq!(result, Err(MediaError::Status(404)));
    let (result, _) = fetch(&transport, "/api/v1/assets/1/thumbnail", MediaClass::Image);
    assert_eq!(result, Err(MediaError::MediaType(None)));
}

#[test]
fn routes_outside_the_daemon_api_never_reach_the_transport() {
    let transport = ChunkingTransport::new(64);
    for route in [
        "https://evil.test/api/v1/effects/prism/cover",
        "//evil.test/api/v1/x",
        "/remote/018f4c36-4a44-7cc9-9f57-0d2e9224d2f1/_d/api/v1/effects/prism/cover",
        "/api/v1/../admin",
        "/api/v1/%2e%2e/admin",
        "/assets/brand/mark.png",
        "blob:https://example.test/abc",
    ] {
        assert!(!is_daemon_media_route(route), "{route}");
        let (result, _) = fetch(&transport, route, MediaClass::Image);
        assert_eq!(result, Err(MediaError::Route), "{route}");
    }
    assert!(transport.requests.borrow().is_empty());
    assert!(is_daemon_media_route(COVER));
    assert!(is_daemon_media_route("/api/v1/displays/d/frame?ts=4"));
}

#[test]
fn a_body_over_the_class_limit_stops_early() {
    let limit = usize::try_from(MAX_IMAGE_BYTES).expect("limit fits");
    let transport = ChunkingTransport::new(1024 * 1024).reply(
        COVER,
        Reply {
            status: 200,
            content_type: Some("image/webp"),
            prefix: webp(64).into(),
            filler: limit,
        },
    );
    let (result, _) = fetch(&transport, COVER, MediaClass::Image);
    assert_eq!(result, Err(MediaError::TooLarge));
    let read = transport.chunk_sizes.borrow().iter().sum::<usize>();
    assert!(read <= limit + 64 * 1024, "read {read} bytes");
}

#[test]
fn a_buffered_transport_gets_the_same_checks() {
    let cover = webp(COVER_BYTES);
    let mut transport = ChunkingTransport::new(1024)
        .reply(COVER, Reply::ok("image/webp", cover.clone()))
        .reply(
            "/api/v1/effects/svg/cover",
            Reply::ok("image/svg+xml", SVG.to_vec()),
        );
    transport.buffered_only = true;
    let (result, collected) = fetch(&transport, COVER, MediaClass::Image);
    assert_eq!(result.expect("buffered cover").len, COVER_BYTES as u64);
    assert_eq!(collected, cover);
    let (result, collected) = fetch(&transport, "/api/v1/effects/svg/cover", MediaClass::Image);
    assert_eq!(
        result,
        Err(MediaError::MediaType(Some("image/svg+xml".to_owned())))
    );
    assert!(collected.is_empty());
}

#[test]
fn media_uses_choose_class_and_caching() {
    assert_eq!(MediaUse::Artwork.class(), MediaClass::Image);
    assert_eq!(MediaUse::LiveStill.class(), MediaClass::Image);
    assert_eq!(MediaUse::Clip.class(), MediaClass::Video);
    assert!(MediaUse::Artwork.caches(COVER_BYTES as u64));
    assert!(!MediaUse::LiveStill.caches(1));
    assert!(MediaUse::Clip.caches(CACHE_MAX_ENTRY_BYTES));
    assert!(!MediaUse::Clip.caches(CACHE_MAX_ENTRY_BYTES + 1));
    assert!(!MediaUse::Artwork.caches(CACHE_MAX_ENTRY_BYTES + 1));
}

#[test]
fn the_lru_evicts_by_age_count_and_bytes() {
    let mut lru = MediaLru::new(2, 100);
    assert!(lru.insert("a".to_owned(), 1, 10));
    assert!(lru.insert("b".to_owned(), 2, 10));
    assert_eq!(lru.get("a"), Some(1));
    assert!(lru.insert("c".to_owned(), 3, 10));
    assert_eq!(lru.get("b"), None, "b was least recently used");
    assert_eq!((lru.len(), lru.bytes()), (2, 20));

    assert!(lru.insert("d".to_owned(), 4, 95));
    assert_eq!(lru.get("a"), None);
    assert_eq!(
        lru.get("c"),
        None,
        "the byte bound evicts past the count bound"
    );
    assert_eq!(lru.get("d"), Some(4));
    assert_eq!(lru.bytes(), 95);

    assert!(!lru.insert("huge".to_owned(), 5, 101));
    assert_eq!(lru.get("huge"), None);
    assert!(lru.insert("d".to_owned(), 6, 5));
    assert_eq!((lru.get("d"), lru.len(), lru.bytes()), (Some(6), 1, 5));
    assert_eq!(lru.remove("d"), Some(6));
    assert!(lru.is_empty());
    assert_eq!(lru.bytes(), 0);
}

#[derive(Default)]
struct CountingWaker(AtomicUsize);

impl Wake for CountingWaker {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn the_limiter_caps_fetches_and_never_strands_a_permit() {
    let counter = Arc::new(CountingWaker::default());
    let waker = Waker::from(Arc::clone(&counter));
    let mut context = Context::from_waker(&waker);
    let limiter = MediaLimiter::new(2);

    let mut first = Box::pin(limiter.acquire());
    let mut second = Box::pin(limiter.acquire());
    let Poll::Ready(first_permit) = first.as_mut().poll(&mut context) else {
        panic!("first permit is free");
    };
    let Poll::Ready(second_permit) = second.as_mut().poll(&mut context) else {
        panic!("second permit is free");
    };
    assert_eq!(limiter.available(), 0);

    let mut abandoned = Box::pin(limiter.acquire());
    let mut third = Box::pin(limiter.acquire());
    assert!(abandoned.as_mut().poll(&mut context).is_pending());
    assert!(third.as_mut().poll(&mut context).is_pending());
    drop(abandoned);

    drop(first_permit);
    assert_eq!(counter.0.load(Ordering::SeqCst), 1, "the live waiter woke");
    assert_eq!(limiter.available(), 0, "the permit passed to the waiter");
    let Poll::Ready(third_permit) = third.as_mut().poll(&mut context) else {
        panic!("third waiter holds the released permit");
    };

    let mut granted_then_dropped = Box::pin(limiter.acquire());
    assert!(
        granted_then_dropped
            .as_mut()
            .poll(&mut context)
            .is_pending()
    );
    drop(second_permit);
    drop(granted_then_dropped);
    assert_eq!(
        limiter.available(),
        1,
        "a granted but dropped waiter returns it"
    );

    drop(third_permit);
    assert_eq!(limiter.available(), 2);
}
