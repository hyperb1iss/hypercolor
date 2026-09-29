//! Daemon media at `<img>` and `<video>` sites.
//!
//! On a local page a daemon media route is an ordinary URL the browser loads
//! itself. Under a Remote bridge no daemon URL is loadable: the bytes travel
//! through the installed transport as an ordinary request, so they ride the
//! bridge's streams and may span any number of frames. They are checked
//! against a raster or video allowlist, both by the declared media type and by
//! the file signature, and become a `blob:` URL the component owns and
//! revokes. A daemon-supplied URL never reaches the network directly.
//!
//! The resolver keeps recent artwork in a byte-bounded LRU and runs at most
//! [`MEDIA_CONCURRENCY`] fetches at once, so media never crowds out control
//! traffic on the bridge.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::rc::{Rc, Weak};
use std::task::{Context, Poll, Waker};

use leptos::prelude::*;

use crate::api::http_transport::{
    HttpBody, HttpBodySource, HttpCancellation, HttpHeader, HttpMethod, HttpRequest,
    HttpRequestBody, HttpStreamError, HttpStreamRequest, HttpStreamResponse, HttpTransport,
};

/// Media fetches in flight at once. A bridged session admits 16 concurrent
/// exchanges and shares a 2 MiB receive credit pool; four media streams at a
/// 256 KiB window hold at most half the pool and leave twelve exchanges for
/// control traffic.
pub const MEDIA_CONCURRENCY: usize = 4;
/// Largest raster image the resolver accepts.
pub const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
/// Largest video clip the resolver accepts. A clip plays from memory once it
/// has arrived whole.
pub const MAX_VIDEO_BYTES: u64 = 256 * 1024 * 1024;
/// Recently resolved media kept for reuse, by count and by total bytes.
pub const CACHE_MAX_ENTRIES: usize = 256;
pub const CACHE_MAX_BYTES: u64 = 48 * 1024 * 1024;
/// Media larger than this is never cached, so one clip cannot flush every
/// cover.
pub const CACHE_MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024;
/// Leading bytes the signature check needs.
const SIGNATURE_BYTES: usize = 12;
const READ_CHUNK_BYTES: usize = 64 * 1024;

const IMAGE_TYPES: [&str; 5] = [
    "image/png",
    "image/apng",
    "image/jpeg",
    "image/webp",
    "image/gif",
];
const VIDEO_TYPES: [&str; 2] = ["video/mp4", "video/webm"];

/// What a media site renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaClass {
    /// A raster image for an `<img>` or a CSS background.
    Image,
    /// A clip for a `<video>`.
    Video,
}

impl MediaClass {
    #[must_use]
    pub const fn max_bytes(self) -> u64 {
        match self {
            Self::Image => MAX_IMAGE_BYTES,
            Self::Video => MAX_VIDEO_BYTES,
        }
    }
}

/// How a site uses its media, which decides the class and the caching.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaUse {
    /// Covers, thumbnails and asset stills: stable per route, so cached.
    Artwork,
    /// A still of something live, such as a display face: never cached.
    LiveStill,
    /// A video clip: cached only while small.
    Clip,
}

impl MediaUse {
    #[must_use]
    pub const fn class(self) -> MediaClass {
        match self {
            Self::Artwork | Self::LiveStill => MediaClass::Image,
            Self::Clip => MediaClass::Video,
        }
    }

    /// Whether a resolved body of `bytes` may enter the cache.
    #[must_use]
    pub const fn caches(self, bytes: u64) -> bool {
        match self {
            Self::Artwork | Self::Clip => bytes <= CACHE_MAX_ENTRY_BYTES,
            Self::LiveStill => false,
        }
    }
}

/// The canonical media type for an allowed `Content-Type`, or `None`. SVG,
/// HTML and anything else outside the class's allowlist are refused.
#[must_use]
pub fn allowed_media_type(class: MediaClass, content_type: &str) -> Option<&'static str> {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let allowed: &[&'static str] = match class {
        MediaClass::Image => &IMAGE_TYPES,
        MediaClass::Video => &VIDEO_TYPES,
    };
    allowed.iter().copied().find(|allowed| *allowed == essence)
}

/// Whether `prefix`, the first bytes of a body, starts the way `media_type`
/// files do. The checks match the ones the daemon uses to classify uploads.
#[must_use]
pub fn signature_matches(media_type: &str, prefix: &[u8]) -> bool {
    match media_type {
        "image/png" | "image/apng" => prefix.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => prefix.starts_with(&[0xff, 0xd8, 0xff]),
        "image/webp" => {
            prefix.len() >= 12 && prefix.starts_with(b"RIFF") && &prefix[8..12] == b"WEBP"
        }
        "image/gif" => prefix.starts_with(b"GIF87a") || prefix.starts_with(b"GIF89a"),
        "video/mp4" => prefix.len() >= 8 && &prefix[4..8] == b"ftyp",
        "video/webm" => prefix.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]),
        _ => false,
    }
}

/// Whether `route` is a daemon API path the resolver may request.
#[must_use]
pub fn is_daemon_media_route(route: &str) -> bool {
    crate::remote_bridge::resolve_remote_api_url_from_base("", route).is_some()
}

/// Why a media route did not resolve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaError {
    /// The route is not a daemon API path.
    Route,
    /// The daemon answered with a status other than 200.
    Status(u16),
    /// The declared media type is missing or outside the allowlist.
    MediaType(Option<String>),
    /// The body does not start like its declared media type.
    Signature,
    /// The body exceeds the class's byte limit.
    TooLarge,
    /// The exchange was cancelled.
    Cancelled,
    /// The transport or the browser failed.
    Transport(String),
}

impl std::fmt::Display for MediaError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Route => formatter.write_str("media route is not a daemon API path"),
            Self::Status(status) => write!(formatter, "media request answered HTTP {status}"),
            Self::MediaType(Some(content_type)) => {
                write!(formatter, "media type {content_type} is not allowed")
            }
            Self::MediaType(None) => formatter.write_str("media response has no content type"),
            Self::Signature => formatter.write_str("media body does not match its media type"),
            Self::TooLarge => formatter.write_str("media body exceeds its size limit"),
            Self::Cancelled => formatter.write_str("media request was cancelled"),
            Self::Transport(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for MediaError {}

impl From<HttpStreamError> for MediaError {
    fn from(error: HttpStreamError) -> Self {
        match error {
            HttpStreamError::Cancelled => Self::Cancelled,
            HttpStreamError::BodyTooLarge => Self::TooLarge,
            error => Self::Transport(error.to_string()),
        }
    }
}

/// A body that passed every check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FetchedMedia {
    /// The canonical allowed media type.
    pub media_type: &'static str,
    /// Bytes handed to the sink.
    pub len: u64,
}

struct NoBody;

impl HttpBodySource for NoBody {
    fn exact_length(&self) -> Option<u64> {
        Some(0)
    }

    fn poll_chunk(
        &mut self,
        _: &mut Context<'_>,
        _: NonZeroUsize,
    ) -> Poll<Result<Option<Vec<u8>>, HttpStreamError>> {
        Poll::Ready(Ok(None))
    }

    fn cancel(&mut self) {}
}

/// Fetch `route` through `transport` and pass its body to `sink` chunk by
/// chunk, once the status, the media type and the signature have been
/// checked. On an error the sink may already hold part of the body, which the
/// caller discards.
///
/// A transport without incremental exchanges answers through its buffered
/// `send`, under the same checks.
///
/// # Errors
///
/// Returns a [`MediaError`] naming the first check that failed.
pub async fn fetch_media(
    transport: &dyn HttpTransport,
    route: &str,
    class: MediaClass,
    sink: &mut dyn FnMut(Vec<u8>),
) -> Result<FetchedMedia, MediaError> {
    if !is_daemon_media_route(route) {
        return Err(MediaError::Route);
    }
    let request = HttpStreamRequest {
        method: HttpMethod::Get,
        path: route.to_owned(),
        headers: Vec::new(),
        body: HttpBody::new(Box::new(NoBody), HttpCancellation::new()),
    };
    match transport.send_stream(request).await {
        Ok(response) => read_streamed(response, class, sink).await,
        Err(HttpStreamError::Unsupported) => {
            let response = transport
                .send(HttpRequest {
                    method: HttpMethod::Get,
                    path: route.to_owned(),
                    headers: Vec::new(),
                    body: HttpRequestBody::Empty,
                })
                .await
                .map_err(|error| MediaError::Transport(error.message))?;
            let media_type = checked_media_type(response.status, &response.headers, class)?;
            let len = response.body.len() as u64;
            if len > class.max_bytes() {
                return Err(MediaError::TooLarge);
            }
            if !signature_matches(media_type, &response.body) {
                return Err(MediaError::Signature);
            }
            if len > 0 {
                sink(response.body);
            }
            Ok(FetchedMedia { media_type, len })
        }
        Err(error) => Err(error.into()),
    }
}

fn checked_media_type(
    status: u16,
    headers: &[HttpHeader],
    class: MediaClass,
) -> Result<&'static str, MediaError> {
    if status != 200 {
        return Err(MediaError::Status(status));
    }
    let content_type = headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("content-type"))
        .map(|header| header.value.as_str());
    content_type
        .and_then(|content_type| allowed_media_type(class, content_type))
        .ok_or_else(|| MediaError::MediaType(content_type.map(str::to_owned)))
}

async fn read_streamed(
    response: HttpStreamResponse,
    class: MediaClass,
    sink: &mut dyn FnMut(Vec<u8>),
) -> Result<FetchedMedia, MediaError> {
    let HttpStreamResponse {
        status,
        headers,
        mut body,
    } = response;
    let media_type = checked_media_type(status, &headers, class)?;
    let limit = class.max_bytes();
    if body.exact_length().is_some_and(|length| length > limit) {
        return Err(MediaError::TooLarge);
    }
    let chunk = NonZeroUsize::new(READ_CHUNK_BYTES).expect("read chunk is positive");
    let mut prefix = Vec::with_capacity(SIGNATURE_BYTES);
    let mut verified = false;
    let mut len = 0_u64;
    while let Some(bytes) = body.read_chunk(chunk).await? {
        len += bytes.len() as u64;
        if len > limit {
            return Err(MediaError::TooLarge);
        }
        if verified {
            sink(bytes);
            continue;
        }
        prefix.extend_from_slice(&bytes);
        if prefix.len() >= SIGNATURE_BYTES {
            if !signature_matches(media_type, &prefix) {
                return Err(MediaError::Signature);
            }
            verified = true;
            sink(std::mem::take(&mut prefix));
        }
    }
    if !verified {
        if !signature_matches(media_type, &prefix) {
            return Err(MediaError::Signature);
        }
        sink(prefix);
    }
    Ok(FetchedMedia { media_type, len })
}

/// A least-recently-used map bounded by entry count and by total bytes.
#[derive(Debug)]
pub struct MediaLru<V> {
    entries: VecDeque<(String, V, u64)>,
    bytes: u64,
    max_entries: usize,
    max_bytes: u64,
}

impl<V: Clone> MediaLru<V> {
    #[must_use]
    pub const fn new(max_entries: usize, max_bytes: u64) -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            max_entries,
            max_bytes,
        }
    }

    /// The value for `key`, now the most recently used.
    pub fn get(&mut self, key: &str) -> Option<V> {
        let index = self.entries.iter().position(|(entry, ..)| entry == key)?;
        let entry = self.entries.remove(index)?;
        let value = entry.1.clone();
        self.entries.push_back(entry);
        Some(value)
    }

    /// Insert `value` weighing `bytes`, evicting the least recently used
    /// entries until both bounds hold. A value heavier than the byte bound
    /// is not kept; returns whether it was.
    pub fn insert(&mut self, key: String, value: V, bytes: u64) -> bool {
        self.remove(&key);
        if bytes > self.max_bytes || self.max_entries == 0 {
            return false;
        }
        self.bytes += bytes;
        self.entries.push_back((key, value, bytes));
        while self.entries.len() > self.max_entries || self.bytes > self.max_bytes {
            let Some((_, _, evicted)) = self.entries.pop_front() else {
                break;
            };
            self.bytes -= evicted;
        }
        true
    }

    pub fn remove(&mut self, key: &str) -> Option<V> {
        let index = self.entries.iter().position(|(entry, ..)| entry == key)?;
        let (_, value, bytes) = self.entries.remove(index)?;
        self.bytes -= bytes;
        Some(value)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// An async counting semaphore for one thread. Waiters are served in order,
/// and a waiter that is dropped, granted or not, never strands a permit.
#[derive(Clone)]
pub struct MediaLimiter(Rc<LimiterState>);

struct LimiterState {
    available: Cell<usize>,
    waiters: RefCell<VecDeque<Weak<Waiter>>>,
}

struct Waiter {
    granted: Cell<bool>,
    waker: RefCell<Option<Waker>>,
}

impl MediaLimiter {
    #[must_use]
    pub fn new(permits: usize) -> Self {
        Self(Rc::new(LimiterState {
            available: Cell::new(permits),
            waiters: RefCell::new(VecDeque::new()),
        }))
    }

    /// Wait for a permit. Dropping the future gives up the place in line.
    #[must_use]
    pub fn acquire(&self) -> MediaAcquire {
        MediaAcquire {
            limiter: self.clone(),
            waiter: None,
            done: false,
        }
    }

    /// Permits not held by anyone.
    #[must_use]
    pub fn available(&self) -> usize {
        self.0.available.get()
    }

    fn release(&self) {
        loop {
            let next = self.0.waiters.borrow_mut().pop_front();
            let Some(next) = next else {
                self.0.available.set(self.0.available.get() + 1);
                return;
            };
            if let Some(waiter) = next.upgrade() {
                waiter.granted.set(true);
                // Release the borrow before waking: an executor may poll the
                // waiter, which stores its waker again, from inside `wake`.
                let waker = waiter.waker.borrow_mut().take();
                if let Some(waker) = waker {
                    waker.wake();
                }
                return;
            }
        }
    }
}

/// Future of [`MediaLimiter::acquire`].
pub struct MediaAcquire {
    limiter: MediaLimiter,
    waiter: Option<Rc<Waiter>>,
    done: bool,
}

impl Future for MediaAcquire {
    type Output = MediaPermit;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        assert!(!this.done, "media permit polled after completion");
        if let Some(waiter) = &this.waiter {
            if waiter.granted.get() {
                this.done = true;
                this.waiter = None;
                return Poll::Ready(MediaPermit {
                    limiter: this.limiter.clone(),
                });
            }
            waiter.waker.replace(Some(context.waker().clone()));
            return Poll::Pending;
        }
        let state = &this.limiter.0;
        state
            .waiters
            .borrow_mut()
            .retain(|waiter| waiter.strong_count() > 0);
        if state.available.get() > 0 && state.waiters.borrow().is_empty() {
            state.available.set(state.available.get() - 1);
            this.done = true;
            return Poll::Ready(MediaPermit {
                limiter: this.limiter.clone(),
            });
        }
        let waiter = Rc::new(Waiter {
            granted: Cell::new(false),
            waker: RefCell::new(Some(context.waker().clone())),
        });
        state.waiters.borrow_mut().push_back(Rc::downgrade(&waiter));
        this.waiter = Some(waiter);
        Poll::Pending
    }
}

impl Drop for MediaAcquire {
    fn drop(&mut self) {
        if let Some(waiter) = self.waiter.take()
            && waiter.granted.get()
        {
            self.limiter.release();
        }
    }
}

/// One fetch's hold on the limiter, released on drop.
pub struct MediaPermit {
    limiter: MediaLimiter,
}

impl Drop for MediaPermit {
    fn drop(&mut self) {
        self.limiter.release();
    }
}

/// What a media site renders from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaSource {
    /// Still resolving.
    Loading,
    /// A URL the element may load: the daemon URL on a local page, a
    /// component-owned `blob:` URL under a bridge.
    Ready(String),
    /// Nothing to show.
    Unavailable,
}

impl MediaSource {
    #[must_use]
    pub fn url(&self) -> Option<String> {
        match self {
            Self::Ready(url) => Some(url.clone()),
            Self::Loading | Self::Unavailable => None,
        }
    }

    #[must_use]
    pub const fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable)
    }
}

/// The source a local page renders for a daemon media route: the daemon
/// URL itself, which the browser loads.
#[must_use]
pub fn direct_media_source(route: &str) -> MediaSource {
    crate::api::client::daemon_url(route)
        .filter(|url| !url.is_empty())
        .map_or(MediaSource::Unavailable, MediaSource::Ready)
}

/// Follow `route` and render it at a media site.
///
/// A local page gets the daemon URL. Under a Remote bridge each route
/// resolves through the transport into a `blob:` URL this call owns: it is
/// revoked when the route changes and when the owning component unmounts,
/// and a fetch still in flight at either moment is cancelled.
pub fn use_media_source(route: Signal<Option<String>>, media_use: MediaUse) -> Signal<MediaSource> {
    #[cfg(target_arch = "wasm32")]
    if crate::remote_bridge::is_available() {
        return browser::use_resolved_media_source(route, media_use);
    }
    let _ = media_use;
    Signal::derive(move || {
        route.get().map_or(MediaSource::Unavailable, |route| {
            direct_media_source(&route)
        })
    })
}

#[cfg(target_arch = "wasm32")]
pub use browser::MediaResolver;

#[cfg(target_arch = "wasm32")]
mod browser {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use futures_util::FutureExt;
    use futures_util::future::{AbortHandle, Abortable, LocalBoxFuture, Shared, WeakShared};
    use leptos::prelude::*;
    use wasm_bindgen::JsValue;

    use super::{
        CACHE_MAX_BYTES, CACHE_MAX_ENTRIES, MEDIA_CONCURRENCY, MediaError, MediaLimiter, MediaLru,
        MediaSource, MediaUse, fetch_media,
    };
    use crate::api::http_transport::HttpTransport;

    thread_local! {
        static SHARED: RefCell<Option<Rc<MediaResolver>>> = const { RefCell::new(None) };
    }

    type Resolution = LocalBoxFuture<'static, Result<web_sys::Blob, MediaError>>;
    type PendingKey = (String, MediaUse);
    type Pending = Rc<RefCell<HashMap<PendingKey, WeakShared<Resolution>>>>;

    /// Resolves daemon media routes into `Blob`s through one transport.
    ///
    /// Sites asking for the same route while it is still arriving share one
    /// fetch. The fetch lives as long as any of them waits for it, so it is
    /// cancelled, and its permit released, once every one of them has gone.
    pub struct MediaResolver {
        transport: Rc<dyn HttpTransport>,
        cache: Rc<RefCell<MediaLru<web_sys::Blob>>>,
        limiter: MediaLimiter,
        pending: Pending,
    }

    impl MediaResolver {
        #[must_use]
        pub fn new(transport: Rc<dyn HttpTransport>) -> Self {
            Self::with_limits(
                transport,
                MEDIA_CONCURRENCY,
                CACHE_MAX_ENTRIES,
                CACHE_MAX_BYTES,
            )
        }

        #[must_use]
        pub fn with_limits(
            transport: Rc<dyn HttpTransport>,
            concurrency: usize,
            cache_entries: usize,
            cache_bytes: u64,
        ) -> Self {
            Self {
                transport,
                cache: Rc::new(RefCell::new(MediaLru::new(cache_entries, cache_bytes))),
                limiter: MediaLimiter::new(concurrency),
                pending: Rc::default(),
            }
        }

        /// The resolver over the installed transport, shared by every site.
        #[must_use]
        pub fn shared() -> Rc<Self> {
            SHARED.with_borrow_mut(|shared| {
                Rc::clone(shared.get_or_insert_with(|| {
                    Rc::new(Self::new(crate::api::client::http_transport()))
                }))
            })
        }

        /// Resolve `route` into a `Blob` typed with its allowed media type.
        ///
        /// # Errors
        ///
        /// Returns the [`MediaError`] of the first check that failed.
        pub async fn resolve(
            &self,
            route: &str,
            media_use: MediaUse,
        ) -> Result<web_sys::Blob, MediaError> {
            if let Some(blob) = cached(&self.cache, route, media_use) {
                return Ok(blob);
            }
            let key = (route.to_owned(), media_use);
            let existing = self
                .pending
                .borrow()
                .get(&key)
                .and_then(WeakShared::upgrade);
            let resolution = existing.unwrap_or_else(|| self.start(key));
            resolution.await
        }

        fn start(&self, key: PendingKey) -> Shared<Resolution> {
            let transport = Rc::clone(&self.transport);
            let cache = Rc::clone(&self.cache);
            let limiter = self.limiter.clone();
            let pending = Rc::clone(&self.pending);
            let pending_key = key.clone();
            let resolution = async move {
                let (route, media_use) = &pending_key;
                let result =
                    fetch_blob(transport.as_ref(), &cache, &limiter, route, *media_use).await;
                pending.borrow_mut().remove(&pending_key);
                result
            }
            .boxed_local()
            .shared();
            if let Some(weak) = resolution.downgrade() {
                self.pending.borrow_mut().insert(key, weak);
            }
            resolution
        }
    }

    fn cached(
        cache: &RefCell<MediaLru<web_sys::Blob>>,
        route: &str,
        media_use: MediaUse,
    ) -> Option<web_sys::Blob> {
        if matches!(media_use, MediaUse::LiveStill) {
            return None;
        }
        cache.borrow_mut().get(route)
    }

    async fn fetch_blob(
        transport: &dyn HttpTransport,
        cache: &RefCell<MediaLru<web_sys::Blob>>,
        limiter: &MediaLimiter,
        route: &str,
        media_use: MediaUse,
    ) -> Result<web_sys::Blob, MediaError> {
        let _permit = limiter.acquire().await;
        if let Some(blob) = cached(cache, route, media_use) {
            return Ok(blob);
        }
        let parts = js_sys::Array::new();
        let fetched = fetch_media(transport, route, media_use.class(), &mut |chunk| {
            parts.push(&js_sys::Uint8Array::from(chunk.as_slice()));
        })
        .await?;
        let options = web_sys::BlobPropertyBag::new();
        options.set_type(fetched.media_type);
        let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)
            .map_err(js_error)?;
        if media_use.caches(fetched.len) {
            cache
                .borrow_mut()
                .insert(route.to_owned(), blob.clone(), fetched.len);
        }
        Ok(blob)
    }

    fn js_error(error: JsValue) -> MediaError {
        MediaError::Transport(
            error
                .as_string()
                .unwrap_or_else(|| "browser refused the media body".to_owned()),
        )
    }

    pub(super) fn use_resolved_media_source(
        route: Signal<Option<String>>,
        media_use: MediaUse,
    ) -> Signal<MediaSource> {
        let source = RwSignal::new(MediaSource::Loading);
        let owned_url = StoredValue::new(None::<String>);
        let task = StoredValue::new(None::<AbortHandle>);
        let generation = StoredValue::new(0_u64);

        let release = move || {
            if let Some(handle) = task.try_update_value(Option::take).flatten() {
                handle.abort();
            }
            if let Some(url) = owned_url.try_update_value(Option::take).flatten() {
                let _ = web_sys::Url::revoke_object_url(&url);
            }
        };

        Effect::new(move |_| {
            let next = route.get();
            release();
            let Some(current) = generation.try_update_value(|value| {
                *value += 1;
                *value
            }) else {
                return;
            };
            let Some(next) = next else {
                source.set(MediaSource::Unavailable);
                return;
            };
            source.set(MediaSource::Loading);
            let (handle, registration) = AbortHandle::new_pair();
            task.set_value(Some(handle));
            let resolver = MediaResolver::shared();
            wasm_bindgen_futures::spawn_local(async move {
                let Ok(result) = Abortable::new(
                    async move { resolver.resolve(&next, media_use).await },
                    registration,
                )
                .await
                else {
                    return;
                };
                if generation.try_get_value() != Some(current) {
                    return;
                }
                let resolved = result
                    .ok()
                    .and_then(|blob| web_sys::Url::create_object_url_with_blob(&blob).ok());
                match resolved {
                    Some(url) => {
                        owned_url.set_value(Some(url.clone()));
                        source.try_set(MediaSource::Ready(url));
                    }
                    None => {
                        source.try_set(MediaSource::Unavailable);
                    }
                }
            });
        });
        on_cleanup(release);
        source.into()
    }
}

#[cfg(test)]
mod tests {
    use super::{MediaSource, direct_media_source};
    use crate::api::client;

    #[test]
    fn local_media_keeps_the_daemon_url_and_waits_for_a_verified_native_route() {
        client::reset_daemon_transport_for_test();
        let route = "/api/v1/effects/prism/cover";
        assert_eq!(
            direct_media_source(route),
            MediaSource::Ready(route.to_owned())
        );

        client::begin_native_daemon_verification();
        assert_eq!(direct_media_source(route), MediaSource::Unavailable);

        client::install_verified_daemon_connection("http://127.0.0.1:9420", Some("protected"));
        assert_eq!(
            direct_media_source(route),
            MediaSource::Ready("http://127.0.0.1:9420/api/v1/effects/prism/cover".to_owned())
        );
        client::reset_daemon_transport_for_test();
    }
}
