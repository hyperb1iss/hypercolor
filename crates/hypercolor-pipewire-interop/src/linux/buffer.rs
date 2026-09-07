use std::mem::{MaybeUninit, align_of, offset_of, size_of};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::slice;

use pipewire::{spa, stream::Stream};

use crate::{
    BufferFault, D4Transform, DequeueOutcome, DmaBufIdentity, MetaFault, PixelCrop, SpaBufferView,
    SpaChunk,
};

const MAX_NATIVE_ENTRIES: u32 = 64;

const _: () = {
    assert!(size_of::<spa::sys::spa_meta>() == 16);
    assert!(offset_of!(spa::sys::spa_meta, data) == 8);
    assert!(size_of::<spa::sys::spa_meta_region>() == 16);
    assert!(size_of::<spa::sys::spa_meta_videotransform>() == 4);
    assert!(size_of::<spa::sys::spa_chunk>() == 16);
    assert!(size_of::<spa::sys::spa_data>() == 40);
    assert!(offset_of!(spa::sys::spa_data, chunk) == 32);
    assert!(size_of::<spa::sys::spa_buffer>() == 24);
};

/// One process callback's authority to dequeue and synchronously visit a buffer.
pub struct ProcessBuffer<'a> {
    stream: &'a Stream,
}

impl<'a> ProcessBuffer<'a> {
    pub(crate) const fn new(stream: &'a Stream) -> Self {
        Self { stream }
    }

    /// Dequeues, validates, visits, and requeues one native buffer.
    pub fn visit<V>(self, visitor: impl FnOnce(SpaBufferView<'_>) -> V) -> DequeueOutcome<V> {
        with_dequeued_buffer(&StreamQueue(self.stream), visitor)
    }
}

trait NativeQueue {
    unsafe fn dequeue(&self) -> *mut pipewire::sys::pw_buffer;
    unsafe fn requeue(&self, buffer: *mut pipewire::sys::pw_buffer);
}

struct StreamQueue<'a>(&'a Stream);

impl NativeQueue for StreamQueue<'_> {
    unsafe fn dequeue(&self) -> *mut pipewire::sys::pw_buffer {
        // SAFETY: the stream callback owns one synchronous dequeue opportunity.
        unsafe { self.0.dequeue_raw_buffer() }
    }

    unsafe fn requeue(&self, buffer: *mut pipewire::sys::pw_buffer) {
        // SAFETY: `buffer` came from this exact stream and is requeued once.
        unsafe { self.0.queue_raw_buffer(buffer) };
    }
}

struct RequeueGuard<'a, Q: NativeQueue> {
    queue: &'a Q,
    buffer: *mut pipewire::sys::pw_buffer,
}

impl<Q: NativeQueue> Drop for RequeueGuard<'_, Q> {
    fn drop(&mut self) {
        // SAFETY: the guard is created only for a successful dequeue and owns
        // the sole requeue for this raw buffer on every return and unwind path.
        unsafe { self.queue.requeue(self.buffer) };
    }
}

fn with_dequeued_buffer<Q, V>(
    queue: &Q,
    visitor: impl FnOnce(SpaBufferView<'_>) -> V,
) -> DequeueOutcome<V>
where
    Q: NativeQueue,
{
    // SAFETY: `NativeQueue` implementations return either null or one buffer
    // whose exclusive dequeue lifetime lasts until the matching requeue.
    let raw = unsafe { queue.dequeue() };
    if raw.is_null() {
        return DequeueOutcome::Empty;
    }
    let _guard = RequeueGuard { queue, buffer: raw };
    // SAFETY: the guard retains exclusive dequeue authority while validation
    // borrows the native buffer and all derived slices.
    let view = match unsafe { validate_buffer(raw) } {
        Ok(view) => view,
        Err(error) => return DequeueOutcome::Faulted(error),
    };
    match catch_unwind(AssertUnwindSafe(|| visitor(view))) {
        Ok(value) => DequeueOutcome::Visited(value),
        Err(_) => DequeueOutcome::VisitorPanicked,
    }
}

unsafe fn validate_buffer<'a>(
    raw: *mut pipewire::sys::pw_buffer,
) -> Result<SpaBufferView<'a>, BufferFault> {
    // SAFETY: the caller holds exclusive dequeue authority for `raw`.
    let native = unsafe { raw.as_ref() }.ok_or(BufferFault::MissingBuffer)?;
    // SAFETY: PipeWire owns the wrapper and its SPA buffer for the dequeue.
    let buffer = unsafe { native.buffer.as_ref() }.ok_or(BufferFault::MissingNativeBuffer)?;
    if buffer.n_datas == 0 {
        return Err(BufferFault::MissingPlane);
    }
    if buffer.n_datas > MAX_NATIVE_ENTRIES || buffer.datas.is_null() {
        return Err(BufferFault::InvalidLayout);
    }
    // SAFETY: PipeWire declares an array of `n_datas` entries. The count is
    // bounded before forming the slice and the dequeue guard retains it.
    let datas = unsafe { slice::from_raw_parts(buffer.datas, buffer.n_datas as usize) };
    let data = &datas[0];
    // SAFETY: a non-null chunk belongs to the first data plane for this dequeue.
    let chunk = unsafe { data.chunk.as_ref() }.ok_or(BufferFault::MissingChunk)?;
    if data.data.is_null() {
        return Err(BufferFault::UnmappedPlane);
    }
    let offset = usize::try_from(chunk.offset).map_err(|_| BufferFault::InvalidChunkBounds)?;
    let size = usize::try_from(chunk.size).map_err(|_| BufferFault::InvalidChunkBounds)?;
    let max_size = usize::try_from(data.maxsize).map_err(|_| BufferFault::InvalidLayout)?;
    if offset.checked_add(size).is_none_or(|end| end > max_size) {
        return Err(BufferFault::InvalidChunkBounds);
    }
    // SAFETY: MAP_BUFFERS guarantees `data` addresses a readable mapping of
    // `maxsize` bytes for the dequeue lifetime. Bounds were validated above.
    let bytes = unsafe { slice::from_raw_parts(data.data.cast::<u8>(), max_size) };
    // SAFETY: the dequeue guard retains the SPA buffer and its bounded metadata
    // array for the lifetime returned by this validator.
    let metas = unsafe { validated_metas(buffer)? };
    Ok(SpaBufferView {
        data: bytes,
        chunk: SpaChunk {
            offset,
            size,
            stride: chunk.stride,
        },
        crop: read_crop(metas),
        transform: read_transform(metas),
        dma_buf_identity: dma_buf_identity(data),
        _callback_thread: std::marker::PhantomData,
    })
}

unsafe fn validated_metas(
    buffer: &spa::sys::spa_buffer,
) -> Result<&[spa::sys::spa_meta], BufferFault> {
    if buffer.n_metas == 0 {
        return Ok(&[]);
    }
    if buffer.n_metas > MAX_NATIVE_ENTRIES || buffer.metas.is_null() {
        return Err(BufferFault::InvalidLayout);
    }
    // SAFETY: PipeWire declares an array of `n_metas` entries. The caller's
    // dequeue guard retains the array and the count is bounded above.
    Ok(unsafe { slice::from_raw_parts(buffer.metas, buffer.n_metas as usize) })
}

fn read_crop(metas: &[spa::sys::spa_meta]) -> Option<Result<PixelCrop, MetaFault>> {
    let native = read_meta::<spa::sys::spa_meta_region>(metas, spa::sys::SPA_META_VideoCrop)?;
    Some(native.and_then(|native| {
        let x = u32::try_from(native.region.position.x).map_err(|_| MetaFault::InvalidCrop)?;
        let y = u32::try_from(native.region.position.y).map_err(|_| MetaFault::InvalidCrop)?;
        let width = native.region.size.width;
        let height = native.region.size.height;
        if width == 0
            || height == 0
            || x.checked_add(width).is_none()
            || y.checked_add(height).is_none()
        {
            return Err(MetaFault::InvalidCrop);
        }
        Ok(PixelCrop {
            x,
            y,
            width,
            height,
        })
    }))
}

fn read_transform(metas: &[spa::sys::spa_meta]) -> Option<Result<D4Transform, MetaFault>> {
    let native =
        read_meta::<spa::sys::spa_meta_videotransform>(metas, spa::sys::SPA_META_VideoTransform)?;
    Some(native.and_then(|native| match native.transform {
        spa::sys::SPA_META_TRANSFORMATION_None => Ok(D4Transform::Identity),
        spa::sys::SPA_META_TRANSFORMATION_90 => Ok(D4Transform::Clockwise90),
        spa::sys::SPA_META_TRANSFORMATION_180 => Ok(D4Transform::Clockwise180),
        spa::sys::SPA_META_TRANSFORMATION_270 => Ok(D4Transform::Clockwise270),
        spa::sys::SPA_META_TRANSFORMATION_Flipped => Ok(D4Transform::Flipped),
        spa::sys::SPA_META_TRANSFORMATION_Flipped90 => Ok(D4Transform::Flipped90),
        spa::sys::SPA_META_TRANSFORMATION_Flipped180 => Ok(D4Transform::Flipped180),
        spa::sys::SPA_META_TRANSFORMATION_Flipped270 => Ok(D4Transform::Flipped270),
        _ => Err(MetaFault::InvalidTransform),
    }))
}

fn read_meta<T: Copy>(
    metas: &[spa::sys::spa_meta],
    meta_type: u32,
) -> Option<Result<T, MetaFault>> {
    let meta = metas.iter().find(|meta| meta.type_ == meta_type)?;
    if usize::try_from(meta.size).map_or(true, |size| size < size_of::<T>()) || meta.data.is_null()
    {
        return Some(Err(MetaFault::Undersized));
    }
    if !(meta.data as usize).is_multiple_of(align_of::<T>()) {
        return Some(Err(MetaFault::Misaligned));
    }
    // SAFETY: the metadata pointer is non-null, aligned, and declares at least
    // one complete `T`; the dequeue guard retains its storage during the copy.
    Some(Ok(unsafe { ptr::read(meta.data.cast::<T>()) }))
}

fn dma_buf_identity(data: &spa::sys::spa_data) -> Result<Option<DmaBufIdentity>, BufferFault> {
    if data.type_ != spa::sys::SPA_DATA_DmaBuf {
        return Ok(None);
    }
    let fd = i32::try_from(data.fd).map_err(|_| BufferFault::InvalidDmaBuf)?;
    if fd < 0 {
        return Err(BufferFault::InvalidDmaBuf);
    }
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `stat` points to writable storage and `fd` is the producer's
    // retained DMA-BUF descriptor for the current dequeue.
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return Err(BufferFault::InvalidDmaBuf);
    }
    // SAFETY: successful `fstat` initialized the entire result structure.
    let stat = unsafe { stat.assume_init() };
    Ok(Some(DmaBufIdentity {
        device: stat.st_dev,
        inode: stat.st_ino,
        plane_offset: data.mapoffset,
        plane_size: data.maxsize,
    }))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::ptr;

    use pipewire::spa;

    use super::{NativeQueue, with_dequeued_buffer};
    use crate::{BufferFault, D4Transform, DequeueOutcome, MetaFault, PixelCrop};

    struct FakeQueue {
        buffer: *mut pipewire::sys::pw_buffer,
        requeues: Cell<u32>,
    }

    impl NativeQueue for FakeQueue {
        unsafe fn dequeue(&self) -> *mut pipewire::sys::pw_buffer {
            self.buffer
        }

        unsafe fn requeue(&self, buffer: *mut pipewire::sys::pw_buffer) {
            assert_eq!(buffer, self.buffer);
            self.requeues.set(self.requeues.get() + 1);
        }
    }

    struct BufferFixture {
        bytes: Box<[u8]>,
        chunk: Box<spa::sys::spa_chunk>,
        data: Box<spa::sys::spa_data>,
        metas: Vec<spa::sys::spa_meta>,
        buffer: Box<spa::sys::spa_buffer>,
        wrapper: Box<pipewire::sys::pw_buffer>,
    }

    impl BufferFixture {
        fn new() -> Self {
            let mut bytes = vec![0_u8; 64].into_boxed_slice();
            let mut chunk = Box::new(spa::sys::spa_chunk {
                offset: 4,
                size: 32,
                stride: 16,
                flags: 0,
            });
            let mut data = Box::new(spa::sys::spa_data {
                type_: spa::sys::SPA_DATA_MemPtr,
                flags: spa::sys::SPA_DATA_FLAG_READABLE,
                fd: -1,
                mapoffset: 0,
                maxsize: u32::try_from(bytes.len()).expect("fixture size fits"),
                data: bytes.as_mut_ptr().cast(),
                chunk: &raw mut *chunk,
            });
            let mut buffer = Box::new(spa::sys::spa_buffer {
                n_metas: 0,
                n_datas: 1,
                metas: ptr::null_mut(),
                datas: &raw mut *data,
            });
            let wrapper = Box::new(pipewire::sys::pw_buffer {
                buffer: &raw mut *buffer,
                user_data: ptr::null_mut(),
                size: 0,
                requested: 0,
                time: 0,
            });
            Self {
                bytes,
                chunk,
                data,
                metas: Vec::new(),
                buffer,
                wrapper,
            }
        }

        fn install_metas(&mut self, metas: Vec<spa::sys::spa_meta>) {
            self.metas = metas;
            self.buffer.n_metas = u32::try_from(self.metas.len()).expect("fixture count fits");
            self.buffer.metas = self.metas.as_mut_ptr();
        }

        fn queue(&mut self) -> FakeQueue {
            self.data.data = self.bytes.as_mut_ptr().cast();
            self.data.chunk = &raw mut *self.chunk;
            self.buffer.datas = &raw mut *self.data;
            self.wrapper.buffer = &raw mut *self.buffer;
            FakeQueue {
                buffer: &raw mut *self.wrapper,
                requeues: Cell::new(0),
            }
        }
    }

    #[test]
    fn dequeue_requeues_exactly_once_on_return_fault_and_panic() {
        let mut fixture = BufferFixture::new();
        let queue = fixture.queue();
        let outcome = with_dequeued_buffer(&queue, |view| view.chunk().size);
        assert!(matches!(outcome, DequeueOutcome::Visited(32)));
        assert_eq!(queue.requeues.get(), 1);

        fixture.buffer.n_datas = 0;
        let queue = fixture.queue();
        fixture.buffer.n_datas = 0;
        let outcome = with_dequeued_buffer(&queue, |_| ());
        assert!(matches!(
            outcome,
            DequeueOutcome::Faulted(BufferFault::MissingPlane)
        ));
        assert_eq!(queue.requeues.get(), 1);

        fixture.buffer.n_datas = 1;
        let queue = fixture.queue();
        let outcome = with_dequeued_buffer(&queue, |_| panic!("visitor boundary"));
        assert!(matches!(outcome, DequeueOutcome::VisitorPanicked));
        assert_eq!(queue.requeues.get(), 1);
    }

    #[test]
    fn metadata_is_copied_into_typed_values() {
        let mut crop = Box::new(spa::sys::spa_meta_region {
            region: spa::sys::spa_region {
                position: spa::sys::spa_point { x: 3, y: 5 },
                size: spa::sys::spa_rectangle {
                    width: 17,
                    height: 19,
                },
            },
        });
        let mut transform = Box::new(spa::sys::spa_meta_videotransform {
            transform: spa::sys::SPA_META_TRANSFORMATION_Flipped90,
        });
        let mut fixture = BufferFixture::new();
        fixture.install_metas(vec![
            spa::sys::spa_meta {
                type_: spa::sys::SPA_META_VideoCrop,
                size: size_of::<spa::sys::spa_meta_region>() as u32,
                data: (&raw mut *crop).cast(),
            },
            spa::sys::spa_meta {
                type_: spa::sys::SPA_META_VideoTransform,
                size: size_of::<spa::sys::spa_meta_videotransform>() as u32,
                data: (&raw mut *transform).cast(),
            },
        ]);
        let queue = fixture.queue();
        let outcome = with_dequeued_buffer(&queue, |view| (view.crop(), view.transform()));
        let DequeueOutcome::Visited((crop, transform)) = outcome else {
            panic!("valid metadata must be visited")
        };
        assert_eq!(
            crop,
            Some(Ok(PixelCrop {
                x: 3,
                y: 5,
                width: 17,
                height: 19,
            }))
        );
        assert_eq!(transform, Some(Ok(D4Transform::Flipped90)));
        assert_eq!(queue.requeues.get(), 1);
    }

    #[test]
    fn malformed_metadata_remains_present_as_a_typed_fault() {
        let mut transform = Box::new(spa::sys::spa_meta_videotransform { transform: 99 });
        let mut fixture = BufferFixture::new();
        fixture.install_metas(vec![spa::sys::spa_meta {
            type_: spa::sys::SPA_META_VideoTransform,
            size: size_of::<spa::sys::spa_meta_videotransform>() as u32,
            data: (&raw mut *transform).cast(),
        }]);
        let queue = fixture.queue();
        let outcome = with_dequeued_buffer(&queue, |view| view.transform());
        assert!(matches!(
            outcome,
            DequeueOutcome::Visited(Some(Err(MetaFault::InvalidTransform)))
        ));
        assert_eq!(queue.requeues.get(), 1);
    }

    #[test]
    fn overflowing_crop_remains_present_as_a_typed_fault() {
        let mut crop = Box::new(spa::sys::spa_meta_region {
            region: spa::sys::spa_region {
                position: spa::sys::spa_point { x: i32::MAX, y: 0 },
                size: spa::sys::spa_rectangle {
                    width: u32::MAX,
                    height: 1,
                },
            },
        });
        let mut fixture = BufferFixture::new();
        fixture.install_metas(vec![spa::sys::spa_meta {
            type_: spa::sys::SPA_META_VideoCrop,
            size: size_of::<spa::sys::spa_meta_region>() as u32,
            data: (&raw mut *crop).cast(),
        }]);
        let queue = fixture.queue();
        let outcome = with_dequeued_buffer(&queue, |view| view.crop());
        assert!(matches!(
            outcome,
            DequeueOutcome::Visited(Some(Err(MetaFault::InvalidCrop)))
        ));
        assert_eq!(queue.requeues.get(), 1);
    }

    #[test]
    fn dma_buf_identity_is_stable_without_exposing_the_fd_number() {
        let file = File::open("/dev/null").expect("fixture fd");
        let mut fixture = BufferFixture::new();
        fixture.data.type_ = spa::sys::SPA_DATA_DmaBuf;
        fixture.data.fd = i64::from(file.as_raw_fd());
        fixture.data.mapoffset = 4096;
        let queue = fixture.queue();
        let first = with_dequeued_buffer(&queue, |view| view.dma_buf_identity());
        let DequeueOutcome::Visited(Ok(Some(first))) = first else {
            panic!("valid DMA-BUF identity must be visited")
        };

        let queue = fixture.queue();
        let second = with_dequeued_buffer(&queue, |view| view.dma_buf_identity());
        assert!(matches!(second, DequeueOutcome::Visited(Ok(Some(id))) if id == first));
    }
}
