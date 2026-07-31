use std::ffi::{CStr, c_void};
use std::mem::{size_of, transmute};
use std::sync::{Arc, OnceLock};

use thiserror::Error;
use windows::Win32::Foundation::{E_OUTOFMEMORY, FreeLibrary, HMODULE};
use windows::Win32::Graphics::Direct3D::Fxc::{
    D3DCOMPILE_ENABLE_STRICTNESS, D3DCOMPILE_OPTIMIZATION_LEVEL3,
};
use windows::Win32::Graphics::Direct3D::{D3D_SHADER_MACRO, ID3DBlob};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_ASYNC_GETDATA_DONOTFLUSH, D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_UNORDERED_ACCESS,
    D3D11_BUFFER_DESC, D3D11_CPU_ACCESS_READ, D3D11_FORMAT_SUPPORT_SHADER_SAMPLE,
    D3D11_FORMAT_SUPPORT_TYPED_UNORDERED_ACCESS_VIEW, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_QUERY_DESC, D3D11_QUERY_EVENT, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, ID3D11Buffer, ID3D11ComputeShader, ID3D11Device,
    ID3D11DeviceContext, ID3D11Query, ID3D11ShaderResourceView, ID3D11Texture2D,
    ID3D11UnorderedAccessView,
};
#[cfg(any(test, feature = "capture-bench"))]
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_UNORM};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{BOOL, HRESULT, Interface, PCSTR, w};

#[cfg(any(test, feature = "capture-bench"))]
use super::PointerState;
use super::{CaptureMetadata, PointerShapeKind, RetainedDesktop};
#[cfg(feature = "capture-bench")]
use crate::CaptureResult;
use crate::shared::{commit_capture_resource, reserve_capture_resource};
use crate::{
    CaptureError, CaptureExtent, CaptureRegion, CaptureResourceAdmission, CaptureResourceKind,
    CaptureResourceLease, DisplayRotation, GpuSurfaceColorPipeline, GpuSurfaceCursorPolicy,
    GpuSurfaceDescriptor, GpuSurfaceFilter, subsample_stride_within, subsampled_extent,
};
#[cfg(any(test, feature = "capture-bench"))]
use windows::Win32::Graphics::Direct3D11::D3D11_BIND_SHADER_RESOURCE;

const READBACK_RING_LEN: usize = 3;
const THREAD_GROUP: u32 = 8;
const SHADER_SOURCE: &[u8] = include_bytes!("reduction.hlsl");

type D3DCompileFn = unsafe extern "system" fn(
    *const c_void,
    usize,
    PCSTR,
    *const D3D_SHADER_MACRO,
    *mut c_void,
    PCSTR,
    PCSTR,
    u32,
    u32,
    *mut *mut c_void,
    *mut *mut c_void,
) -> HRESULT;

#[derive(Debug, Error)]
pub(super) enum GpuReductionError {
    #[error("{message}")]
    Operation { message: String },
    #[error("{context}: {message}")]
    Windows {
        context: &'static str,
        message: String,
    },
    #[error("{context}: RGBA8 byte size overflows for {width}x{height}")]
    SizeOverflow {
        context: &'static str,
        width: u32,
        height: u32,
    },
    #[error("{context}: could not reserve {requested_bytes} bytes: {message}")]
    ResourceExhausted {
        context: &'static str,
        requested_bytes: usize,
        message: String,
    },
    #[error(
        "{operation} admission mismatch: expected {expected_kind:?}/{expected_bytes} bytes, got {actual_kind:?}/{actual_bytes} bytes"
    )]
    ResourceAdmissionMismatch {
        operation: &'static str,
        expected_kind: CaptureResourceKind,
        expected_bytes: u64,
        actual_kind: CaptureResourceKind,
        actual_bytes: u64,
    },
}

impl GpuReductionError {
    fn operation(message: impl Into<String>) -> Self {
        Self::Operation {
            message: message.into(),
        }
    }

    fn windows(context: &'static str, error: impl std::fmt::Display) -> Self {
        Self::Windows {
            context,
            message: error.to_string(),
        }
    }

    fn resource_exhausted(
        context: &'static str,
        requested_bytes: usize,
        error: impl std::fmt::Display,
    ) -> Self {
        Self::ResourceExhausted {
            context,
            requested_bytes,
            message: error.to_string(),
        }
    }

    fn capture_resource(error: CaptureError) -> Self {
        match error {
            CaptureError::ResourceExhausted {
                operation,
                requested_bytes,
            } => Self::ResourceExhausted {
                context: operation,
                requested_bytes,
                message: "capture resource admission rejected the quote".to_owned(),
            },
            CaptureError::ResourceAdmissionMismatch {
                operation,
                expected_kind,
                expected_bytes,
                actual_kind,
                actual_bytes,
            } => Self::ResourceAdmissionMismatch {
                operation,
                expected_kind,
                expected_bytes,
                actual_kind,
                actual_bytes,
            },
            other => Self::Operation {
                message: other.to_string(),
            },
        }
    }

    pub(super) fn as_capture_error(&self) -> Option<CaptureError> {
        match self {
            Self::ResourceExhausted {
                context,
                requested_bytes,
                ..
            } => Some(CaptureError::ResourceExhausted {
                operation: context,
                requested_bytes: *requested_bytes,
            }),
            Self::SizeOverflow {
                context,
                width,
                height,
            } => Some(CaptureError::GeometryOverflow {
                operation: context,
                width: *width,
                height: *height,
            }),
            Self::ResourceAdmissionMismatch {
                operation,
                expected_kind,
                expected_bytes,
                actual_kind,
                actual_bytes,
            } => Some(CaptureError::ResourceAdmissionMismatch {
                operation,
                expected_kind: *expected_kind,
                expected_bytes: *expected_bytes,
                actual_kind: *actual_kind,
                actual_bytes: *actual_bytes,
            }),
            Self::Operation { .. } | Self::Windows { .. } => None,
        }
    }
}

pub(super) fn checked_rgba_len(
    width: u32,
    height: u32,
    context: &'static str,
) -> Result<usize, GpuReductionError> {
    usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(GpuReductionError::SizeOverflow {
            context,
            width,
            height,
        })
}

pub(super) fn checked_rgba_row_pitch(
    width: u32,
    height: u32,
    context: &'static str,
) -> Result<u32, GpuReductionError> {
    width.checked_mul(4).ok_or(GpuReductionError::SizeOverflow {
        context,
        width,
        height,
    })
}

fn admit_vec_len(
    buffer: &mut Vec<u8>,
    requested_len: usize,
    context: &'static str,
) -> Result<(), GpuReductionError> {
    if requested_len <= buffer.capacity() {
        return Ok(());
    }
    buffer
        .try_reserve(requested_len.saturating_sub(buffer.len()))
        .map_err(|error| GpuReductionError::resource_exhausted(context, requested_len, error))
}

#[cfg(feature = "capture-bench")]
fn public_capture_error(error: GpuReductionError) -> CaptureError {
    error
        .as_capture_error()
        .unwrap_or_else(|| CaptureError::windows("run D3D11 capture reduction", error))
}

struct ShaderBytecode {
    reduce: Vec<u8>,
    reduce_exact: Vec<u8>,
    publish_surface: Vec<u8>,
}

static SHADER_BYTECODE: OnceLock<Result<ShaderBytecode, String>> = OnceLock::new();

struct Library(HMODULE);

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: this handle came from LoadLibraryW and remains owned here.
        let _ = unsafe { FreeLibrary(self.0) };
    }
}

fn compiled_shaders() -> Result<&'static ShaderBytecode, GpuReductionError> {
    SHADER_BYTECODE
        .get_or_init(|| {
            // SAFETY: the static UTF-16 name is NUL-terminated.
            let library = Library(
                unsafe { LoadLibraryW(w!("d3dcompiler_47.dll")) }
                    .map_err(|error| format!("load d3dcompiler_47.dll: {error}"))?,
            );
            // SAFETY: the module is live and the ASCII function name is
            // NUL-terminated.
            let entry = unsafe { GetProcAddress(library.0, PCSTR(c"D3DCompile".as_ptr().cast())) }
                .ok_or_else(|| "resolve D3DCompile: entry point missing".to_owned())?;
            // SAFETY: D3DCompile has the documented ABI represented by the
            // local function type. The library stays loaded through both calls.
            let compile: D3DCompileFn = unsafe { transmute(entry) };
            Ok(ShaderBytecode {
                reduce: compile_entry(compile, c"reduce_desktop")?,
                reduce_exact: compile_entry(compile, c"reduce_desktop_exact")?,
                publish_surface: compile_entry(compile, c"publish_surface_exact")?,
            })
        })
        .as_ref()
        .map_err(|message| GpuReductionError::operation(message.clone()))
}

fn compile_entry(compile: D3DCompileFn, entry: &'static CStr) -> Result<Vec<u8>, String> {
    let mut code = std::ptr::null_mut();
    let mut errors = std::ptr::null_mut();
    // SAFETY: all pointers describe live immutable source/name buffers or
    // caller-owned out-pointers, and the function pointer was resolved from
    // d3dcompiler_47.dll with the exact D3DCompile ABI.
    let result = unsafe {
        compile(
            SHADER_SOURCE.as_ptr().cast(),
            SHADER_SOURCE.len(),
            PCSTR(c"hypercolor-capture-reduction.hlsl".as_ptr().cast()),
            std::ptr::null(),
            std::ptr::null_mut(),
            PCSTR(entry.as_ptr().cast()),
            PCSTR(c"cs_5_0".as_ptr().cast()),
            D3DCOMPILE_ENABLE_STRICTNESS | D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            &mut errors,
        )
    };
    let error_text = blob_text(errors);
    if result.is_err() {
        return Err(format!(
            "compile {}: {}{}",
            entry.to_string_lossy(),
            result,
            error_text.map_or_else(String::new, |text| format!(" ({text})"))
        ));
    }
    if code.is_null() {
        return Err("D3DCompile returned no shader bytecode".to_owned());
    }

    // SAFETY: successful D3DCompile returned one owned ID3DBlob reference.
    let blob = unsafe { ID3DBlob::from_raw(code) };
    // SAFETY: the blob owns GetBufferSize readable bytes until it is dropped.
    let bytes = unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    };
    Ok(bytes.to_vec())
}

fn blob_text(raw: *mut c_void) -> Option<String> {
    if raw.is_null() {
        return None;
    }
    // SAFETY: D3DCompile returned one owned ID3DBlob reference in `raw`.
    let blob = unsafe { ID3DBlob::from_raw(raw) };
    // SAFETY: the blob owns GetBufferSize readable bytes until it is dropped.
    let bytes = unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    };
    Some(
        String::from_utf8_lossy(bytes)
            .trim_matches('\0')
            .trim()
            .to_owned(),
    )
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ShaderParams {
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
    stride: u32,
    rotation: u32,
    pointer_kind: u32,
    pointer_visible: u32,
    pointer_x: i32,
    pointer_y: i32,
    pointer_width: u32,
    pointer_height: u32,
    region_x: u32,
    region_y: u32,
    region_width: u32,
    region_height: u32,
    filter: u32,
    color_pipeline: u32,
    cursor_policy: u32,
    padding: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResourceKey {
    width: u32,
    height: u32,
    output_width: u32,
    output_height: u32,
    stride: u32,
    format: DXGI_FORMAT,
    region: CaptureRegion,
}

struct PendingFrame {
    metadata: CaptureMetadata,
}

struct ReadbackSlot {
    staging: ID3D11Texture2D,
    query: ID3D11Query,
    pending: Option<PendingFrame>,
    progress_kicked: bool,
}

struct Resources {
    key: ResourceKey,
    reduced: ID3D11Texture2D,
    reduced_uav: ID3D11UnorderedAccessView,
    slots: Box<[ReadbackSlot]>,
    write_index: usize,
    read_index: usize,
    _resource_lease: Option<Arc<dyn CaptureResourceLease>>,
}

pub(super) enum SubmitOutcome {
    Submitted,
    Busy,
}

pub(super) struct ReducedFrame {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) metadata: CaptureMetadata,
    pub(super) bytes: usize,
}

pub(super) struct GpuReducer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    reduce_shader: ID3D11ComputeShader,
    params: ID3D11Buffer,
    resources: Option<Resources>,
    resource_admission: Option<Arc<dyn CaptureResourceAdmission>>,
    _constant_buffer_lease: Option<Arc<dyn CaptureResourceLease>>,
    #[cfg(test)]
    poll_failure: Option<InjectedPollFailure>,
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(super) enum InjectedPollFailure {
    Query,
    Map,
}

impl GpuReducer {
    pub(super) fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        resource_admission: Arc<dyn CaptureResourceAdmission>,
    ) -> Result<Self, GpuReductionError> {
        let bytecode = compiled_shaders()?;
        let reduce_shader = create_compute_shader(device, &bytecode.reduce)?;
        let constant_buffer_bytes = u64::try_from(constant_buffer_byte_len())
            .map_err(|_| GpuReductionError::operation("constant buffer size exceeds u64"))?;
        let reservation = reserve_capture_resource(
            resource_admission.as_ref(),
            CaptureResourceKind::CompatibilityReductionConstantBuffer,
            constant_buffer_bytes,
            "reserve compatibility reduction constant buffer",
        )
        .map_err(GpuReductionError::capture_resource)?;
        let params = create_constant_buffer(device)?;
        let constant_buffer_lease = commit_capture_resource(
            reservation,
            constant_buffer_bytes,
            "commit compatibility reduction constant buffer",
        )
        .map_err(GpuReductionError::capture_resource)?;
        Ok(Self {
            device: device.clone(),
            context: context.clone(),
            reduce_shader,
            params,
            resources: None,
            resource_admission: Some(resource_admission),
            _constant_buffer_lease: Some(constant_buffer_lease),
            #[cfg(test)]
            poll_failure: None,
        })
    }

    pub(super) fn new_exact(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        native_extent: CaptureExtent,
        source_format: DXGI_FORMAT,
        descriptor: &GpuSurfaceDescriptor,
        slot_count: u32,
    ) -> Result<Self, GpuReductionError> {
        let bytecode = compiled_shaders()?;
        let reduce_shader = create_compute_shader(device, &bytecode.reduce_exact)?;
        let params = create_constant_buffer(device)?;
        let key = ResourceKey {
            width: native_extent.width(),
            height: native_extent.height(),
            output_width: descriptor.output_extent().width(),
            output_height: descriptor.output_extent().height(),
            stride: 0,
            format: source_format,
            region: descriptor.source_region(),
        };
        let resources = create_resources(device, key, slot_count)?;
        Ok(Self {
            device: device.clone(),
            context: context.clone(),
            reduce_shader,
            params,
            resources: Some(resources),
            resource_admission: None,
            _constant_buffer_lease: None,
            #[cfg(test)]
            poll_failure: None,
        })
    }

    #[cfg(test)]
    pub(super) fn actual_metadata_byte_len_for_test(&self) -> u64 {
        self.resources.as_ref().map_or(0, |resources| {
            resources
                .slots
                .len()
                .checked_mul(size_of::<ReadbackSlot>())
                .and_then(|bytes| u64::try_from(bytes).ok())
                .expect("prepared readback slot metadata was quoted before allocation")
        })
    }

    pub(super) fn submit(
        &mut self,
        clean: &RetainedDesktop,
        pointer_resource: Option<&super::gpu_surface::PointerResource>,
        requested_extent: CaptureExtent,
        metadata: CaptureMetadata,
    ) -> Result<SubmitOutcome, GpuReductionError> {
        self.ensure_resources(&clean.texture, requested_extent, metadata.region)?;
        let context = self.context.clone();
        let params_buffer = self.params.clone();

        let resources = self
            .resources
            .as_mut()
            .expect("resource initialization succeeds before submission");
        if resources.slots[resources.write_index].pending.is_some() {
            return Ok(SubmitOutcome::Busy);
        }
        let pointer = &metadata.pointer;
        let rotation = metadata.rotation;
        let shape = pointer.shape.as_ref().filter(|_| pointer.visible);
        let params = ShaderParams {
            source_width: resources.key.width,
            source_height: resources.key.height,
            output_width: resources.key.output_width,
            output_height: resources.key.output_height,
            stride: resources.key.stride,
            rotation: rotation_code(rotation),
            pointer_kind: shape.map_or(0, |shape| pointer_kind_code(shape.kind)),
            pointer_visible: u32::from(shape.is_some()),
            pointer_x: pointer.position_x,
            pointer_y: pointer.position_y,
            pointer_width: shape.map_or(0, |shape| shape.width),
            pointer_height: shape.map_or(0, |shape| shape.visible_height()),
            region_x: resources.key.region.origin_x(),
            region_y: resources.key.region.origin_y(),
            region_width: resources.key.region.width(),
            region_height: resources.key.region.height(),
            filter: filter_code(GpuSurfaceFilter::Area),
            color_pipeline: color_pipeline_code(GpuSurfaceColorPipeline::PreserveEncoded),
            cursor_policy: cursor_policy_code(GpuSurfaceCursorPolicy::Include),
            padding: 0,
        };
        update_params(&context, &params_buffer, &params);
        let srvs = [
            Some(clean.srv.clone()),
            shape.and(pointer_resource.map(|pointer| pointer.srv.clone())),
        ];
        let uavs = [Some(resources.reduced_uav.clone())];
        // SAFETY: resources match the shader contract and dispatch dimensions
        // are clipped to the configured output extent in HLSL.
        unsafe {
            self.context
                .CSSetConstantBuffers(0, Some(&[Some(self.params.clone())]));
            self.context.CSSetShaderResources(0, Some(&srvs));
            self.context
                .CSSetUnorderedAccessViews(0, 1, Some(uavs.as_ptr()), None);
            self.context
                .CSSetShader(&self.reduce_shader, None::<&[Option<_>]>);
            self.context.Dispatch(
                resources.key.output_width.div_ceil(THREAD_GROUP),
                resources.key.output_height.div_ceil(THREAD_GROUP),
                1,
            );
        }
        unbind_compute_views(&context);

        let slot = &mut resources.slots[resources.write_index];
        // SAFETY: staging shares the reduced texture descriptor except for
        // usage/bind flags, and the event query belongs to this context.
        unsafe {
            self.context.CopyResource(&slot.staging, &resources.reduced);
            self.context.End(&slot.query);
        }
        slot.pending = Some(PendingFrame { metadata });
        slot.progress_kicked = false;
        resources.write_index = (resources.write_index + 1) % resources.slots.len();
        Ok(SubmitOutcome::Submitted)
    }

    pub(super) fn submit_exact(
        &mut self,
        clean: &RetainedDesktop,
        pointer_resource: Option<&super::gpu_surface::PointerResource>,
        descriptor: &GpuSurfaceDescriptor,
        metadata: CaptureMetadata,
    ) -> Result<SubmitOutcome, GpuReductionError> {
        let context = self.context.clone();
        let params_buffer = self.params.clone();
        let resources = self
            .resources
            .as_mut()
            .expect("exact reduction resources are prepared with the plan");
        let mut source_desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: GetDesc fills a live caller-owned descriptor.
        unsafe { clean.texture.GetDesc(&mut source_desc) };
        if source_desc.Width != resources.key.width
            || source_desc.Height != resources.key.height
            || source_desc.Format != resources.key.format
            || descriptor.output_extent().width() != resources.key.output_width
            || descriptor.output_extent().height() != resources.key.output_height
            || descriptor.source_region() != resources.key.region
        {
            return Err(GpuReductionError::operation(
                "exact GPU reduction descriptor or source changed after preparation",
            ));
        }
        if resources.slots[resources.write_index].pending.is_some() {
            return Ok(SubmitOutcome::Busy);
        }
        let pointer = &metadata.pointer;
        let shape = pointer.shape.as_ref().filter(|_| pointer.visible);
        let pointer_resource = pointer_resource.filter(|resource| {
            shape.is_some_and(|shape| {
                resource.generation == pointer.shape_generation
                    && resource.width == shape.width
                    && resource.height == shape.visible_height()
            })
        });
        let params = ShaderParams {
            source_width: resources.key.width,
            source_height: resources.key.height,
            output_width: resources.key.output_width,
            output_height: resources.key.output_height,
            stride: 0,
            rotation: rotation_code(metadata.rotation),
            pointer_kind: shape.map_or(0, |shape| pointer_kind_code(shape.kind)),
            pointer_visible: u32::from(
                descriptor.cursor() == GpuSurfaceCursorPolicy::Include
                    && pointer_resource.is_some(),
            ),
            pointer_x: pointer.position_x,
            pointer_y: pointer.position_y,
            pointer_width: shape.map_or(0, |shape| shape.width),
            pointer_height: shape.map_or(0, |shape| shape.visible_height()),
            region_x: resources.key.region.origin_x(),
            region_y: resources.key.region.origin_y(),
            region_width: resources.key.region.width(),
            region_height: resources.key.region.height(),
            filter: filter_code(descriptor.filter()),
            color_pipeline: color_pipeline_code(descriptor.color_pipeline()),
            cursor_policy: cursor_policy_code(descriptor.cursor()),
            padding: 0,
        };
        update_params(&context, &params_buffer, &params);
        let srvs = [
            Some(clean.srv.clone()),
            pointer_resource.map(|resource| resource.srv.clone()),
        ];
        let uavs = [Some(resources.reduced_uav.clone())];
        // SAFETY: immutable plan resources match the exact shader contract.
        unsafe {
            self.context
                .CSSetConstantBuffers(0, Some(&[Some(self.params.clone())]));
            self.context.CSSetShaderResources(0, Some(&srvs));
            self.context
                .CSSetUnorderedAccessViews(0, 1, Some(uavs.as_ptr()), None);
            self.context
                .CSSetShader(&self.reduce_shader, None::<&[Option<_>]>);
            self.context.Dispatch(
                resources.key.output_width.div_ceil(THREAD_GROUP),
                resources.key.output_height.div_ceil(THREAD_GROUP),
                1,
            );
        }
        unbind_compute_views(&context);

        let slot = &mut resources.slots[resources.write_index];
        // SAFETY: staging and reduced resources share exact texture geometry.
        unsafe {
            self.context.CopyResource(&slot.staging, &resources.reduced);
            self.context.End(&slot.query);
        }
        slot.pending = Some(PendingFrame { metadata });
        slot.progress_kicked = false;
        resources.write_index = (resources.write_index + 1) % resources.slots.len();
        Ok(SubmitOutcome::Submitted)
    }

    #[cfg(any(test, feature = "capture-bench"))]
    pub(super) fn poll(
        &mut self,
        rgba: &mut Vec<u8>,
    ) -> Result<Option<ReducedFrame>, GpuReductionError> {
        if !self.query_ready()? {
            return Ok(None);
        }
        self.read_ready(rgba).map(Some)
    }

    pub(super) fn poll_preallocated(
        &mut self,
        rgba: &mut [u8],
    ) -> Result<Option<ReducedFrame>, GpuReductionError> {
        if !self.query_ready()? {
            return Ok(None);
        }
        self.read_ready_preallocated(rgba).map(Some)
    }

    pub(super) fn output_byte_len(&self) -> Result<Option<usize>, GpuReductionError> {
        self.resources
            .as_ref()
            .map(|resources| {
                checked_rgba_len(
                    resources.key.output_width,
                    resources.key.output_height,
                    "reserve reduced readback",
                )
            })
            .transpose()
    }

    fn query_ready(&mut self) -> Result<bool, GpuReductionError> {
        #[cfg(test)]
        if matches!(self.poll_failure, Some(InjectedPollFailure::Query)) {
            self.poll_failure = None;
            return Err(GpuReductionError::operation("injected query failure"));
        }
        let Some(resources) = self.resources.as_mut() else {
            return Ok(false);
        };
        let slot = &mut resources.slots[resources.read_index];
        if slot.pending.is_none() {
            return Ok(false);
        }
        let mut ready = BOOL::default();
        let flags = query_poll_flags(&mut slot.progress_kicked);
        // SAFETY: the query is live, and `ready` supplies the documented BOOL
        // output storage. The first poll flushes queued work without waiting;
        // later polls remain non-blocking and avoid redundant flushes.
        unsafe {
            self.context.GetData(
                &slot.query,
                Some((&raw mut ready).cast()),
                u32::try_from(size_of::<BOOL>()).unwrap_or(u32::MAX),
                flags,
            )
        }
        .map_err(|error| GpuReductionError::windows("poll reduction query", error))?;
        Ok(ready.as_bool())
    }

    #[cfg(any(test, feature = "capture-bench"))]
    fn read_ready(&mut self, rgba: &mut Vec<u8>) -> Result<ReducedFrame, GpuReductionError> {
        let resources = self
            .resources
            .as_ref()
            .expect("a ready query belongs to initialized resources");
        let output_len = checked_rgba_len(
            resources.key.output_width,
            resources.key.output_height,
            "allocate reduced readback",
        )?;
        admit_vec_len(rgba, output_len, "allocate reduced readback")?;
        rgba.resize(output_len, 0);
        self.read_ready_preallocated(rgba)
    }

    fn read_ready_preallocated(
        &mut self,
        rgba: &mut [u8],
    ) -> Result<ReducedFrame, GpuReductionError> {
        #[cfg(test)]
        if matches!(self.poll_failure, Some(InjectedPollFailure::Map)) {
            self.poll_failure = None;
            return Err(GpuReductionError::operation("injected map failure"));
        }
        let resources = self
            .resources
            .as_mut()
            .expect("a ready query belongs to initialized resources");
        let slot = &mut resources.slots[resources.read_index];
        let pending = slot
            .pending
            .as_ref()
            .expect("a ready query has pending frame metadata");
        let output_len = checked_rgba_len(
            resources.key.output_width,
            resources.key.output_height,
            "allocate reduced readback",
        )?;
        if rgba.len() != output_len {
            return Err(GpuReductionError::operation(
                "preallocated reduced readback has invalid length",
            ));
        }
        let row_bytes =
            checked_rgba_len(resources.key.output_width, 1, "validate reduced row pitch")?;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: staging is CPU-readable and its event query completed.
        unsafe {
            self.context
                .Map(&slot.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(|error| GpuReductionError::windows("map reduced staging texture", error))?;
        let row_pitch = mapped.RowPitch as usize;
        let mapped_len = row_pitch
            .checked_mul(resources.key.output_height as usize)
            .filter(|len| *len <= isize::MAX as usize);
        if row_pitch < row_bytes || mapped_len.is_none() || mapped.pData.is_null() {
            // SAFETY: pairs with the successful Map above.
            unsafe { self.context.Unmap(&slot.staging, 0) };
            return Err(GpuReductionError::operation(
                "mapped reduction surface has invalid row geometry",
            ));
        }
        // SAFETY: Map exposes RowPitch bytes for each output row until Unmap.
        let source = unsafe {
            std::slice::from_raw_parts(
                mapped.pData.cast::<u8>(),
                mapped_len.expect("mapped length was validated above"),
            )
        };
        for row in 0..resources.key.output_height as usize {
            let source_start = row * row_pitch;
            let target_start = row * row_bytes;
            rgba[target_start..target_start + row_bytes]
                .copy_from_slice(&source[source_start..source_start + row_bytes]);
        }
        // SAFETY: pairs with the successful Map above.
        unsafe { self.context.Unmap(&slot.staging, 0) };

        let metadata = pending.metadata.clone();
        slot.pending = None;
        slot.progress_kicked = false;
        resources.read_index = (resources.read_index + 1) % resources.slots.len();
        Ok(ReducedFrame {
            width: resources.key.output_width,
            height: resources.key.output_height,
            metadata,
            bytes: output_len,
        })
    }

    fn ensure_resources(
        &mut self,
        texture: &ID3D11Texture2D,
        requested_extent: CaptureExtent,
        region: CaptureRegion,
    ) -> Result<(), GpuReductionError> {
        let mut source_desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: GetDesc fills a live caller-owned descriptor.
        unsafe { texture.GetDesc(&mut source_desc) };
        let key = resource_key(&source_desc, requested_extent, region)?;
        if self
            .resources
            .as_ref()
            .is_some_and(|resources| resources.key == key)
        {
            return Ok(());
        }

        let resource_admission = self
            .resource_admission
            .as_ref()
            .expect("compatibility reduction carries source resource admission");
        let retained_bytes = checked_resource_retained_bytes(key, READBACK_RING_LEN as u32)?;
        let reservation = reserve_capture_resource(
            resource_admission.as_ref(),
            CaptureResourceKind::CompatibilityReductionTextures,
            retained_bytes,
            "reserve compatibility reduction textures",
        )
        .map_err(GpuReductionError::capture_resource)?;
        let mut replacement = create_resources(&self.device, key, READBACK_RING_LEN as u32)?;
        replacement._resource_lease = Some(
            commit_capture_resource(
                reservation,
                retained_bytes,
                "commit compatibility reduction textures",
            )
            .map_err(GpuReductionError::capture_resource)?,
        );
        replacement.write_index = 0;
        replacement.read_index = 0;
        self.resources = Some(replacement);
        Ok(())
    }
}

fn query_poll_flags(progress_kicked: &mut bool) -> u32 {
    if *progress_kicked {
        D3D11_ASYNC_GETDATA_DONOTFLUSH.0.cast_unsigned()
    } else {
        *progress_kicked = true;
        0
    }
}

fn update_params(context: &ID3D11DeviceContext, buffer: &ID3D11Buffer, params: &ShaderParams) {
    // SAFETY: the constant buffer matches ShaderParams byte size and the
    // source pointer remains live through this immediate copy call.
    unsafe {
        context.UpdateSubresource(
            buffer,
            0,
            None,
            (params as *const ShaderParams).cast(),
            0,
            0,
        )
    };
}

fn unbind_compute_views(context: &ID3D11DeviceContext) {
    let srvs = [None, None];
    let uavs = [None];
    // SAFETY: null views explicitly release compute-stage resource binds.
    unsafe {
        context.CSSetShaderResources(0, Some(&srvs));
        context.CSSetUnorderedAccessViews(0, 1, Some(uavs.as_ptr()), None);
    }
}

fn resource_key(
    desc: &D3D11_TEXTURE2D_DESC,
    requested_extent: CaptureExtent,
    region: CaptureRegion,
) -> Result<ResourceKey, GpuReductionError> {
    if desc.Width == 0 || desc.Height == 0 {
        return Err(GpuReductionError::operation(
            "duplicated texture has an empty extent",
        ));
    }
    if !region.fits_within(desc.Width, desc.Height) {
        return Err(GpuReductionError::operation(
            "capture region is outside the duplicated texture",
        ));
    }
    let stride = subsample_stride_within(region.width(), region.height(), requested_extent);
    let output_width = subsampled_extent(region.width(), stride);
    let output_height = subsampled_extent(region.height(), stride);
    checked_rgba_len(output_width, output_height, "admit reduction geometry")?;
    Ok(ResourceKey {
        width: desc.Width,
        height: desc.Height,
        output_width,
        output_height,
        stride,
        format: desc.Format,
        region,
    })
}

fn source_desc(key: ResourceKey) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: key.width,
        Height: key.height,
        MipLevels: 1,
        ArraySize: 1,
        Format: key.format,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: 0,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    }
}

fn create_resources(
    device: &ID3D11Device,
    key: ResourceKey,
    slot_count: u32,
) -> Result<Resources, GpuReductionError> {
    require_format_support(
        device,
        key.format,
        D3D11_FORMAT_SUPPORT_SHADER_SAMPLE.0.cast_unsigned(),
        "shader-sample",
    )?;
    require_format_support(
        device,
        DXGI_FORMAT_R8G8B8A8_UNORM,
        D3D11_FORMAT_SUPPORT_TYPED_UNORDERED_ACCESS_VIEW
            .0
            .cast_unsigned(),
        "typed-UAV",
    )?;
    let source = source_desc(key);
    let reduced_desc = D3D11_TEXTURE2D_DESC {
        Width: key.output_width,
        Height: key.output_height,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        BindFlags: D3D11_BIND_UNORDERED_ACCESS.0.cast_unsigned(),
        ..source
    };
    let reduced = create_texture(device, &reduced_desc, None)?;
    let reduced_uav = create_uav(device, &reduced)?;
    let staging_desc = D3D11_TEXTURE2D_DESC {
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0.cast_unsigned(),
        ..reduced_desc
    };
    let slot_count = usize::try_from(slot_count)
        .map_err(|_| GpuReductionError::operation("readback slot count exceeds usize"))?;
    let mut slots = Vec::new();
    slots.try_reserve_exact(slot_count).map_err(|error| {
        GpuReductionError::resource_exhausted(
            "allocate GPU reduction readback slots",
            slot_count.saturating_mul(size_of::<ReadbackSlot>()),
            error,
        )
    })?;
    for _ in 0..slot_count {
        slots.push(ReadbackSlot {
            staging: create_texture(device, &staging_desc, None)?,
            query: create_event_query(device)?,
            pending: None,
            progress_kicked: false,
        });
    }
    Ok(Resources {
        key,
        reduced,
        reduced_uav,
        slots: slots.into_boxed_slice(),
        write_index: 0,
        read_index: 0,
        _resource_lease: None,
    })
}

fn checked_resource_retained_bytes(
    key: ResourceKey,
    slot_count: u32,
) -> Result<u64, GpuReductionError> {
    let output_bytes = u64::from(key.output_width)
        .checked_mul(u64::from(key.output_height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(GpuReductionError::SizeOverflow {
            context: "account compatibility reduction textures",
            width: key.output_width,
            height: key.output_height,
        })?;
    let texture_bytes = output_bytes
        .checked_mul(u64::from(slot_count).saturating_add(1))
        .ok_or(GpuReductionError::SizeOverflow {
            context: "account compatibility reduction texture ring",
            width: key.output_width,
            height: key.output_height,
        })?;
    let metadata_bytes = u64::try_from(size_of::<ReadbackSlot>())
        .ok()
        .and_then(|slot| u64::from(slot_count).checked_mul(slot))
        .ok_or_else(|| GpuReductionError::operation("reduction slot metadata exceeds u64"))?;
    texture_bytes
        .checked_add(metadata_bytes)
        .ok_or_else(|| GpuReductionError::operation("reduction retained bytes overflow"))
}

pub(super) const fn constant_buffer_byte_len() -> usize {
    size_of::<ShaderParams>()
}

pub(super) fn readback_slot_metadata_byte_len(
    descriptor_count: usize,
    slots_per_descriptor: std::num::NonZeroU32,
) -> Result<u64, CaptureError> {
    let slot_count = usize::try_from(slots_per_descriptor.get()).map_err(|_| {
        CaptureError::ResourceExhausted {
            operation: "quote GPU reduction readback slot metadata",
            requested_bytes: usize::MAX,
        }
    })?;
    let total_slots =
        descriptor_count
            .checked_mul(slot_count)
            .ok_or(CaptureError::ResourceExhausted {
                operation: "quote GPU reduction readback slot metadata",
                requested_bytes: usize::MAX,
            })?;
    total_slots
        .checked_mul(size_of::<ReadbackSlot>())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or(CaptureError::ResourceExhausted {
            operation: "quote GPU reduction readback slot metadata",
            requested_bytes: usize::MAX,
        })
}

const fn filter_code(filter: GpuSurfaceFilter) -> u32 {
    match filter {
        GpuSurfaceFilter::Nearest => 0,
        GpuSurfaceFilter::Bilinear => 1,
        GpuSurfaceFilter::Area => 2,
    }
}

const fn color_pipeline_code(pipeline: GpuSurfaceColorPipeline) -> u32 {
    match pipeline {
        GpuSurfaceColorPipeline::PreserveEncoded => 0,
        GpuSurfaceColorPipeline::LinearSdr => 1,
        GpuSurfaceColorPipeline::ToneMapHdrToSdr => 2,
    }
}

const fn cursor_policy_code(policy: GpuSurfaceCursorPolicy) -> u32 {
    match policy {
        GpuSurfaceCursorPolicy::Exclude => 0,
        GpuSurfaceCursorPolicy::Include => 1,
    }
}

fn require_format_support(
    device: &ID3D11Device,
    format: DXGI_FORMAT,
    required: u32,
    usage: &'static str,
) -> Result<(), GpuReductionError> {
    // SAFETY: format support is a read-only device query.
    let support = unsafe { device.CheckFormatSupport(format) }
        .map_err(|error| GpuReductionError::windows("query texture format support", error))?;
    if support & required != required {
        return Err(GpuReductionError::operation(format!(
            "format {format:?} lacks {usage} support"
        )));
    }
    Ok(())
}

fn create_compute_shader(
    device: &ID3D11Device,
    bytecode: &[u8],
) -> Result<ID3D11ComputeShader, GpuReductionError> {
    let mut shader = None;
    // SAFETY: bytecode came from D3DCompile for cs_5_0 and the out-pointer is
    // live for the duration of the call.
    unsafe { device.CreateComputeShader(bytecode, None, Some(&mut shader)) }.map_err(|error| {
        classify_allocation_error("create capture compute shader", bytecode.len(), error)
    })?;
    shader.ok_or_else(|| GpuReductionError::operation("compute shader creation returned no shader"))
}

pub(super) fn create_surface_compute_shader(
    device: &ID3D11Device,
) -> Result<ID3D11ComputeShader, GpuReductionError> {
    create_compute_shader(device, &compiled_shaders()?.publish_surface)
}

pub(super) fn create_constant_buffer(
    device: &ID3D11Device,
) -> Result<ID3D11Buffer, GpuReductionError> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: u32::try_from(size_of::<ShaderParams>()).unwrap_or(u32::MAX),
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0.cast_unsigned(),
        CPUAccessFlags: 0,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let mut buffer = None;
    // SAFETY: descriptor is valid and the out-pointer remains live.
    unsafe { device.CreateBuffer(&desc, None, Some(&mut buffer)) }.map_err(|error| {
        classify_allocation_error("create reduction constants", desc.ByteWidth as usize, error)
    })?;
    buffer
        .ok_or_else(|| GpuReductionError::operation("constant buffer creation returned no buffer"))
}

pub(super) fn create_texture(
    device: &ID3D11Device,
    desc: &D3D11_TEXTURE2D_DESC,
    initial: Option<&D3D11_SUBRESOURCE_DATA>,
) -> Result<ID3D11Texture2D, GpuReductionError> {
    let requested_bytes = checked_rgba_len(desc.Width, desc.Height, "admit D3D11 texture")?;
    if let Some(initial) = initial {
        let minimum_pitch =
            checked_rgba_row_pitch(desc.Width, desc.Height, "admit D3D11 texture row pitch")?;
        if initial.pSysMem.is_null() || initial.SysMemPitch < minimum_pitch {
            return Err(GpuReductionError::operation(
                "initial D3D11 texture data has invalid row geometry",
            ));
        }
    }
    let mut texture = None;
    // SAFETY: descriptor and optional initial data are live through the call;
    // the out-pointer remains valid.
    unsafe { device.CreateTexture2D(desc, initial.map(std::ptr::from_ref), Some(&mut texture)) }
        .map_err(|error| {
            classify_allocation_error("create reduction texture", requested_bytes, error)
        })?;
    texture.ok_or_else(|| GpuReductionError::operation("texture creation returned no texture"))
}

pub(super) fn create_srv(
    device: &ID3D11Device,
    texture: &ID3D11Texture2D,
) -> Result<ID3D11ShaderResourceView, GpuReductionError> {
    let requested_bytes = texture_rgba_bytes(texture, "create reduction SRV")?;
    let mut view = None;
    // SAFETY: the texture supports shader-resource binding and the default
    // view spans its only subresource.
    unsafe { device.CreateShaderResourceView(texture, None, Some(&mut view)) }.map_err(
        |error| classify_allocation_error("create reduction SRV", requested_bytes, error),
    )?;
    view.ok_or_else(|| GpuReductionError::operation("SRV creation returned no view"))
}

pub(super) fn create_uav(
    device: &ID3D11Device,
    texture: &ID3D11Texture2D,
) -> Result<ID3D11UnorderedAccessView, GpuReductionError> {
    let requested_bytes = texture_rgba_bytes(texture, "create reduction UAV")?;
    let mut view = None;
    // SAFETY: the texture format was checked for typed UAV support and the
    // default view spans its only subresource.
    unsafe { device.CreateUnorderedAccessView(texture, None, Some(&mut view)) }.map_err(
        |error| classify_allocation_error("create reduction UAV", requested_bytes, error),
    )?;
    view.ok_or_else(|| GpuReductionError::operation("UAV creation returned no view"))
}

fn create_event_query(device: &ID3D11Device) -> Result<ID3D11Query, GpuReductionError> {
    let desc = D3D11_QUERY_DESC {
        Query: D3D11_QUERY_EVENT,
        MiscFlags: 0,
    };
    let mut query = None;
    // SAFETY: the event query descriptor and out-pointer remain live.
    unsafe { device.CreateQuery(&desc, Some(&mut query)) }.map_err(|error| {
        classify_allocation_error(
            "create reduction event query",
            size_of::<D3D11_QUERY_DESC>(),
            error,
        )
    })?;
    query.ok_or_else(|| GpuReductionError::operation("query creation returned no query"))
}

fn texture_rgba_bytes(
    texture: &ID3D11Texture2D,
    context: &'static str,
) -> Result<usize, GpuReductionError> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: GetDesc fills a caller-owned descriptor and cannot fail.
    unsafe { texture.GetDesc(&mut desc) };
    checked_rgba_len(desc.Width, desc.Height, context)
}

fn classify_allocation_error(
    context: &'static str,
    requested_bytes: usize,
    error: windows::core::Error,
) -> GpuReductionError {
    if error.code() == E_OUTOFMEMORY {
        GpuReductionError::resource_exhausted(context, requested_bytes, error)
    } else {
        GpuReductionError::windows(context, error)
    }
}

#[cfg(feature = "capture-bench")]
pub fn classify_allocation_pressure_for_test(
    context: &'static str,
    requested_bytes: usize,
) -> CaptureError {
    classify_allocation_error(
        context,
        requested_bytes,
        windows::core::Error::from_hresult(E_OUTOFMEMORY),
    )
    .as_capture_error()
    .expect("E_OUTOFMEMORY always maps to a typed capture error")
}

pub(super) fn normalized_pointer(
    shape: &super::PointerShape,
) -> Result<Vec<u8>, GpuReductionError> {
    let output_len = checked_rgba_len(
        shape.width,
        shape.visible_height(),
        "normalize pointer texture",
    )?;
    let mut pixels = Vec::new();
    admit_vec_len(&mut pixels, output_len, "normalize pointer texture")?;
    for y in 0..shape.visible_height() as usize {
        for x in 0..shape.width as usize {
            match shape.kind {
                PointerShapeKind::Color | PointerShapeKind::MaskedColor => {
                    let pixel = shape.bgra_pixel(x, y).unwrap_or([0; 4]);
                    pixels.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                }
                PointerShapeKind::Monochrome => {
                    let byte = x / 8;
                    let bit = 0x80_u8 >> (x % 8);
                    let row = y * shape.pitch as usize;
                    let xor_row = (y + shape.visible_height() as usize) * shape.pitch as usize;
                    let and = shape
                        .bytes
                        .get(row + byte)
                        .is_some_and(|value| value & bit != 0);
                    let xor = shape
                        .bytes
                        .get(xor_row + byte)
                        .is_some_and(|value| value & bit != 0);
                    pixels.extend_from_slice(&[
                        if and { 0xFF } else { 0 },
                        if xor { 0xFF } else { 0 },
                        0,
                        0xFF,
                    ]);
                }
            }
        }
    }
    Ok(pixels)
}

pub(super) const fn pointer_kind_code(kind: PointerShapeKind) -> u32 {
    match kind {
        PointerShapeKind::Color => 0,
        PointerShapeKind::MaskedColor => 1,
        PointerShapeKind::Monochrome => 2,
    }
}

pub(super) const fn rotation_code(rotation: DisplayRotation) -> u32 {
    match rotation {
        DisplayRotation::Identity => 0,
        DisplayRotation::Clockwise90 => 1,
        DisplayRotation::Clockwise180 => 2,
        DisplayRotation::Clockwise270 => 3,
    }
}

/// One instrumented GPU reduction used by the Windows Criterion target.
#[cfg(feature = "capture-bench")]
#[derive(Clone, Copy, Debug)]
pub struct ReductionBenchmarkSample {
    /// CPU time to enqueue the duplicated-surface copy into the clean texture.
    pub acquisition_enqueue: std::time::Duration,
    /// CPU time to enqueue cursor composition, reduction, and staging copy.
    pub analysis_enqueue: std::time::Duration,
    /// Time until the event query reported the reduced staging slot ready.
    pub wait: std::time::Duration,
    /// Time to map and copy the tightly packed reduced plane.
    pub map: std::time::Duration,
    /// Native source bytes represented by the acquisition.
    pub source_bytes: u64,
    /// Bytes actually read back from the reduced surface.
    pub readback_bytes: u64,
}

/// End-to-end 120 Hz acquisition / 60 Hz analysis cadence measurements.
#[cfg(feature = "capture-bench")]
pub struct CaptureCadenceReport {
    /// CPU enqueue duration for every native acquisition copy.
    pub acquisition_enqueue: Vec<std::time::Duration>,
    /// Submit-to-readback duration for every completed analysis frame.
    pub analysis_latency: Vec<std::time::Duration>,
    /// Acquisition enqueue operations exceeding the 120 Hz budget.
    pub acquisition_misses: u64,
    /// Analysis completions exceeding the 60 Hz budget.
    pub analysis_misses: u64,
    /// Analysis submissions coalesced into the latest clean desktop.
    pub ring_busy: u64,
    /// Native bytes represented by all acquisitions.
    pub source_bytes: u64,
    /// Reduced bytes actually copied to CPU memory.
    pub readback_bytes: u64,
}

/// Stable-resource D3D11 harness for the Windows capture Criterion target.
#[cfg(feature = "capture-bench")]
pub struct CaptureReductionBenchmark {
    context: ID3D11DeviceContext,
    reducer: GpuReducer,
    source: ID3D11Texture2D,
    clean: RetainedDesktop,
    pointer: PointerState,
    requested_extent: CaptureExtent,
    source_bytes: u64,
    source_width: u32,
    source_height: u32,
    output: Vec<u8>,
    sequence: u64,
}

#[cfg(feature = "capture-bench")]
impl CaptureReductionBenchmark {
    /// Create a hardware-backed stable-resource reduction fixture.
    pub fn new(width: u32, height: u32, requested_extent: CaptureExtent) -> CaptureResult<Self> {
        use windows::Win32::Graphics::Direct3D::{
            D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0,
        };
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice,
        };
        use windows::Win32::Graphics::Dxgi::IDXGIAdapter;

        let rgba_len = checked_rgba_len(width, height, "allocate benchmark source")
            .map_err(public_capture_error)?;
        let mut rgba = Vec::new();
        admit_vec_len(&mut rgba, rgba_len, "allocate benchmark source")
            .map_err(public_capture_error)?;
        for index in 0..rgba_len / 4 {
            let value = index as u8;
            rgba.extend_from_slice(&[value, value.wrapping_add(47), value.wrapping_add(109), 0xFF]);
        }
        let mut device = None;
        let mut context = None;
        // SAFETY: the hardware path takes no explicit adapter, and all input
        // slices and out-pointers remain live for the call.
        unsafe {
            D3D11CreateDevice(
                None::<&IDXGIAdapter>,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(|error| CaptureError::windows("create benchmark D3D11 device", error))?;
        let device = device.ok_or_else(|| {
            CaptureError::windows("create benchmark D3D11 device", "D3D11 returned no device")
        })?;
        let context = context.ok_or_else(|| {
            CaptureError::windows(
                "create benchmark D3D11 context",
                "D3D11 returned no context",
            )
        })?;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: 0,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let initial = D3D11_SUBRESOURCE_DATA {
            pSysMem: rgba.as_ptr().cast(),
            SysMemPitch: checked_rgba_row_pitch(width, height, "create benchmark source")
                .map_err(public_capture_error)?,
            SysMemSlicePitch: 0,
        };
        let source =
            create_texture(&device, &desc, Some(&initial)).map_err(public_capture_error)?;
        let pointer = PointerState::default();
        let clean_desc = D3D11_TEXTURE2D_DESC {
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0.cast_unsigned(),
            ..desc
        };
        let clean_texture =
            create_texture(&device, &clean_desc, None).map_err(public_capture_error)?;
        let clean = RetainedDesktop {
            srv: create_srv(&device, &clean_texture).map_err(public_capture_error)?,
            texture: clean_texture,
            metadata: synthetic_metadata(
                width,
                height,
                &pointer,
                DisplayRotation::Identity,
                CaptureRegion::full(width, height),
                1,
            ),
            _resource_lease: None,
        };
        let mut reducer = GpuReducer::new(
            &device,
            &context,
            crate::shared::default_capture_resource_admission(),
        )
        .map_err(public_capture_error)?;
        let region = CaptureRegion::full(width, height);
        reducer
            .ensure_resources(&clean.texture, requested_extent, region)
            .map_err(public_capture_error)?;
        Ok(Self {
            context,
            reducer,
            source,
            clean,
            pointer,
            requested_extent,
            source_bytes: u64::try_from(rgba_len).unwrap_or(u64::MAX),
            source_width: width,
            source_height: height,
            output: Vec::new(),
            sequence: 0,
        })
    }

    /// Run one acquisition-copy, reduction, wait, and reduced-map sample.
    pub fn sample(&mut self) -> Result<ReductionBenchmarkSample, String> {
        use std::time::Instant;

        let acquisition_started = Instant::now();
        // SAFETY: the source and clean textures have identical descriptors and
        // belong to the same device.
        unsafe { self.context.CopyResource(&self.clean.texture, &self.source) };
        let acquisition_enqueue = acquisition_started.elapsed();

        let reduction_started = Instant::now();
        self.sequence = self.sequence.wrapping_add(1).max(1);
        let metadata = synthetic_metadata(
            self.source_width,
            self.source_height,
            &self.pointer,
            DisplayRotation::Identity,
            CaptureRegion::full(self.source_width, self.source_height),
            self.sequence,
        );
        self.clean.metadata = metadata.clone();
        match self
            .reducer
            .submit(&self.clean, None, self.requested_extent, metadata)
            .map_err(|error| error.to_string())?
        {
            SubmitOutcome::Submitted => {}
            SubmitOutcome::Busy => return Err("benchmark readback ring is busy".to_owned()),
        }
        let analysis_enqueue = reduction_started.elapsed();
        let wait_started = Instant::now();
        while !self
            .reducer
            .query_ready()
            .map_err(|error| error.to_string())?
        {
            std::hint::spin_loop();
        }
        let wait = wait_started.elapsed();
        let map_started = Instant::now();
        let frame = self
            .reducer
            .read_ready(&mut self.output)
            .map_err(|error| error.to_string())?;
        let map = map_started.elapsed();
        Ok(ReductionBenchmarkSample {
            acquisition_enqueue,
            analysis_enqueue,
            wait,
            map,
            source_bytes: self.source_bytes,
            readback_bytes: frame.bytes as u64,
        })
    }

    /// Exercise the production ring at real 120 Hz acquisition and 60 Hz
    /// analysis cadence.
    pub fn run_cadence(&mut self, acquisitions: u32) -> Result<CaptureCadenceReport, String> {
        use std::collections::HashMap;
        use std::time::{Duration, Instant};

        let acquisition_budget = Duration::from_secs_f64(1.0 / 120.0);
        let analysis_budget = Duration::from_secs_f64(1.0 / 60.0);
        let started = Instant::now();
        let mut acquisition_enqueue = Vec::with_capacity(acquisitions as usize);
        let mut analysis_latency = Vec::with_capacity(acquisitions.div_ceil(2) as usize);
        let mut submitted: HashMap<u64, Instant> = HashMap::new();
        let mut ring_busy = 0_u64;
        let mut readback_bytes = 0_u64;

        for tick in 0..acquisitions {
            let target = started + acquisition_budget.mul_f64(f64::from(tick));
            if let Some(delay) = target.checked_duration_since(Instant::now()) {
                std::thread::sleep(delay);
            }
            self.sequence = self.sequence.wrapping_add(1).max(1);
            let metadata = synthetic_metadata(
                self.source_width,
                self.source_height,
                &self.pointer,
                DisplayRotation::Identity,
                CaptureRegion::full(self.source_width, self.source_height),
                self.sequence,
            );
            let acquisition_started = Instant::now();
            // SAFETY: source and canonical clean share an exact descriptor.
            unsafe { self.context.CopyResource(&self.clean.texture, &self.source) };
            self.clean.metadata = metadata.clone();
            acquisition_enqueue.push(acquisition_started.elapsed());

            if let Some(frame) = self
                .reducer
                .poll(&mut self.output)
                .map_err(|error| error.to_string())?
            {
                if let Some(submitted_at) = submitted.remove(&frame.metadata.sequence) {
                    analysis_latency.push(submitted_at.elapsed());
                }
                readback_bytes = readback_bytes.saturating_add(frame.bytes as u64);
            }

            if tick % 2 == 1 {
                let sequence = metadata.sequence;
                let submitted_at = Instant::now();
                match self
                    .reducer
                    .submit(&self.clean, None, self.requested_extent, metadata)
                    .map_err(|error| error.to_string())?
                {
                    SubmitOutcome::Submitted => {
                        submitted.insert(sequence, submitted_at);
                        if let Some(frame) = self
                            .reducer
                            .poll(&mut self.output)
                            .map_err(|error| error.to_string())?
                        {
                            if let Some(submitted_at) = submitted.remove(&frame.metadata.sequence) {
                                analysis_latency.push(submitted_at.elapsed());
                            }
                            readback_bytes = readback_bytes.saturating_add(frame.bytes as u64);
                        }
                    }
                    SubmitOutcome::Busy => ring_busy = ring_busy.saturating_add(1),
                }
            }
            if let Some(frame) = self
                .reducer
                .poll(&mut self.output)
                .map_err(|error| error.to_string())?
            {
                if let Some(submitted_at) = submitted.remove(&frame.metadata.sequence) {
                    analysis_latency.push(submitted_at.elapsed());
                }
                readback_bytes = readback_bytes.saturating_add(frame.bytes as u64);
            }
        }

        let drain_deadline = Instant::now() + Duration::from_secs(2);
        while !submitted.is_empty() && Instant::now() < drain_deadline {
            if let Some(frame) = self
                .reducer
                .poll(&mut self.output)
                .map_err(|error| error.to_string())?
            {
                if let Some(submitted_at) = submitted.remove(&frame.metadata.sequence) {
                    analysis_latency.push(submitted_at.elapsed());
                }
                readback_bytes = readback_bytes.saturating_add(frame.bytes as u64);
            } else {
                std::thread::yield_now();
            }
        }
        if !submitted.is_empty() {
            return Err(format!(
                "{} cadence reductions did not complete within two seconds",
                submitted.len()
            ));
        }

        let acquisition_misses = acquisition_enqueue
            .iter()
            .filter(|duration| **duration > acquisition_budget)
            .count() as u64;
        let analysis_misses = analysis_latency
            .iter()
            .filter(|duration| **duration > analysis_budget)
            .count() as u64;
        Ok(CaptureCadenceReport {
            acquisition_enqueue,
            analysis_latency,
            acquisition_misses,
            analysis_misses,
            ring_busy,
            source_bytes: self.source_bytes.saturating_mul(u64::from(acquisitions)),
            readback_bytes,
        })
    }
}

#[cfg(any(test, feature = "capture-bench"))]
fn synthetic_metadata(
    width: u32,
    height: u32,
    pointer: &PointerState,
    rotation: DisplayRotation,
    region: CaptureRegion,
    sequence: u64,
) -> CaptureMetadata {
    CaptureMetadata {
        source_id: std::sync::Arc::from("synthetic"),
        topology_generation: 1,
        sequence,
        captured_at: std::time::Instant::now(),
        cursor: pointer.cursor_info(width, height, rotation),
        pointer: pointer.clone(),
        source_width: width,
        source_height: height,
        origin_x: 0,
        origin_y: 0,
        rotation,
        source_color_space: crate::GpuSurfaceSourceColorSpace::RgbFullG22P709,
        region,
    }
}

#[cfg(test)]
pub(super) fn compile_shaders_for_test() -> Result<(), GpuReductionError> {
    compiled_shaders().map(|_| ())
}

#[cfg(test)]
pub(super) fn normalized_pointer_for_test(shape: &super::PointerShape) -> Vec<u8> {
    normalized_pointer(shape).expect("pointer fixture geometry must be allocatable")
}

#[cfg(test)]
pub(super) fn query_poll_flags_for_test() -> (u32, u32) {
    let mut progress_kicked = false;
    let first = query_poll_flags(&mut progress_kicked);
    let second = query_poll_flags(&mut progress_kicked);
    (first, second)
}

#[cfg(test)]
pub(super) fn invalid_shader_is_rejected_for_test() -> Result<bool, GpuReductionError> {
    let (device, _) = test_device()?;
    Ok(create_compute_shader(&device, b"not shader bytecode").is_err())
}

#[cfg(test)]
pub(super) fn ring_pressure_is_bounded_for_test() -> Result<(usize, bool), GpuReductionError> {
    let (device, context) = test_device()?;
    let source = test_source(&device, &[10, 20, 30, 0xFF], 1, 1)?;
    let pointer = PointerState::default();
    let clean = test_clean(
        &device,
        &source,
        synthetic_metadata(
            1,
            1,
            &pointer,
            DisplayRotation::Identity,
            CaptureRegion::full(1, 1),
            1,
        ),
    )?;
    let mut reducer = GpuReducer::new(
        &device,
        &context,
        crate::shared::default_capture_resource_admission(),
    )?;
    for submission in 0..=READBACK_RING_LEN {
        let metadata = synthetic_metadata(
            1,
            1,
            &pointer,
            DisplayRotation::Identity,
            CaptureRegion::full(1, 1),
            submission as u64 + 1,
        );
        let outcome = reducer.submit(
            &clean,
            None,
            CaptureExtent::try_new(1, u32::MAX).expect("test extent"),
            metadata,
        )?;
        if submission < READBACK_RING_LEN {
            if !matches!(outcome, SubmitOutcome::Submitted) {
                return Ok((submission, false));
            }
        } else {
            let pending = reducer.resources.as_ref().map_or(0, |resources| {
                resources
                    .slots
                    .iter()
                    .filter(|slot| slot.pending.is_some())
                    .count()
            });
            return Ok((pending, matches!(outcome, SubmitOutcome::Busy)));
        }
    }
    unreachable!("ring pressure loop always reaches the busy probe")
}

#[cfg(test)]
pub(super) fn ring_busy_keeps_latest_clean_metadata_for_test()
-> Result<(u64, u64, CaptureRegion), GpuReductionError> {
    let (device, context) = test_device()?;
    let source = test_source(&device, &[10, 20, 30, 0xFF], 1, 1)?;
    let pointer = PointerState::default();
    let region = CaptureRegion::full(1, 1);
    let mut reducer = GpuReducer::new(
        &device,
        &context,
        crate::shared::default_capture_resource_admission(),
    )?;
    let mut clean = test_clean(
        &device,
        &source,
        synthetic_metadata(1, 1, &pointer, DisplayRotation::Identity, region, 1),
    )?;
    for sequence in 1..=4 {
        let metadata =
            synthetic_metadata(1, 1, &pointer, DisplayRotation::Identity, region, sequence);
        clean.metadata = metadata.clone();
        reducer.submit(
            &clean,
            None,
            CaptureExtent::try_new(1, u32::MAX).expect("test extent"),
            metadata,
        )?;
    }
    let resources = reducer.resources.as_ref().expect("ring resources exist");
    let pending_sequence = resources.slots[resources.read_index]
        .pending
        .as_ref()
        .expect("oldest slot is pending")
        .metadata
        .sequence;
    Ok((
        pending_sequence,
        clean.metadata.sequence,
        clean.metadata.region,
    ))
}

#[cfg(test)]
pub(super) fn poll_failure_preserves_clean_metadata_for_test(
    failure: InjectedPollFailure,
) -> Result<(u64, CaptureRegion), GpuReductionError> {
    use std::time::{Duration, Instant};

    let (device, context) = test_device()?;
    let pixels = [10, 20, 30, 0xFF].repeat(15);
    let source = test_source(&device, &pixels, 5, 3)?;
    let pointer = PointerState::default();
    let region = CaptureRegion::new(1, 1, 4, 2).expect("fixture region is non-empty");
    let mut reducer = GpuReducer::new(
        &device,
        &context,
        crate::shared::default_capture_resource_admission(),
    )?;
    let metadata = synthetic_metadata(5, 3, &pointer, DisplayRotation::Identity, region, 41);
    let clean = test_clean(&device, &source, metadata.clone())?;
    reducer.submit(
        &clean,
        None,
        CaptureExtent::try_new(3, u32::MAX).expect("test extent"),
        metadata,
    )?;
    reducer.poll_failure = Some(failure);
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut output = Vec::new();
    loop {
        match reducer.poll(&mut output) {
            Err(_) => break,
            Ok(Some(_)) => {
                return Err(GpuReductionError::operation(
                    "injected poll failure unexpectedly delivered a frame",
                ));
            }
            Ok(None) if Instant::now() < deadline => std::thread::yield_now(),
            Ok(None) => {
                return Err(GpuReductionError::operation(
                    "injected poll failure did not trigger within two seconds",
                ));
            }
        }
    }
    Ok((clean.metadata.sequence, clean.metadata.region))
}

#[cfg(test)]
pub(super) fn unsupported_source_format_is_rejected_for_test() -> Result<bool, GpuReductionError> {
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN;

    let (device, _) = test_device()?;
    Ok(require_format_support(
        &device,
        DXGI_FORMAT_UNKNOWN,
        D3D11_FORMAT_SUPPORT_SHADER_SAMPLE.0.cast_unsigned(),
        "shader-sample",
    )
    .is_err())
}

#[cfg(test)]
pub(super) fn region_changes_resource_identity_for_test() -> Result<bool, GpuReductionError> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: 7,
        Height: 5,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        ..D3D11_TEXTURE2D_DESC::default()
    };
    let first = resource_key(
        &desc,
        CaptureExtent::try_new(3, u32::MAX).expect("test extent"),
        CaptureRegion::new(1, 1, 5, 3).expect("fixture region is non-empty"),
    )?;
    let second = resource_key(
        &desc,
        CaptureExtent::try_new(3, u32::MAX).expect("test extent"),
        CaptureRegion::new(2, 1, 5, 3).expect("fixture region is non-empty"),
    )?;
    Ok(first != second)
}

#[cfg(test)]
pub(super) fn reduce_fixture(
    bgra: &[u8],
    width: u32,
    height: u32,
    max_width: u32,
    pointer: &PointerState,
    rotation: DisplayRotation,
) -> Result<Vec<u8>, GpuReductionError> {
    reduce_region_fixture(
        bgra,
        width,
        height,
        max_width,
        pointer,
        rotation,
        CaptureRegion::full(width, height),
    )
}

#[cfg(test)]
pub(super) fn reduce_exact_fixture(
    bgra: &[u8],
    native_width: u32,
    native_height: u32,
    descriptor: &GpuSurfaceDescriptor,
) -> Result<Vec<u8>, GpuReductionError> {
    let (device, context) = test_device()?;
    let source = test_source(&device, bgra, native_width, native_height)?;
    let pointer = PointerState::default();
    let metadata = synthetic_metadata(
        native_width,
        native_height,
        &pointer,
        descriptor.source_rotation(),
        descriptor.source_region(),
        41,
    );
    let clean = test_clean(&device, &source, metadata.clone())?;
    let mut reducer = GpuReducer::new_exact(
        &device,
        &context,
        CaptureExtent::try_new(native_width, native_height)
            .map_err(|error| GpuReductionError::operation(error.to_string()))?,
        DXGI_FORMAT_B8G8R8A8_UNORM,
        descriptor,
        3,
    )?;
    match reducer.submit_exact(&clean, None, descriptor, metadata)? {
        SubmitOutcome::Submitted => poll_test_reduction(&mut reducer),
        SubmitOutcome::Busy => Err(GpuReductionError::operation(
            "fresh exact reduction ring was unexpectedly busy",
        )),
    }
}

#[cfg(test)]
pub(super) fn reduce_region_fixture(
    bgra: &[u8],
    width: u32,
    height: u32,
    max_width: u32,
    pointer: &PointerState,
    rotation: DisplayRotation,
    region: CaptureRegion,
) -> Result<Vec<u8>, GpuReductionError> {
    let (device, context) = test_device()?;
    let source = test_source(&device, bgra, width, height)?;
    let mut reducer = GpuReducer::new(
        &device,
        &context,
        crate::shared::default_capture_resource_admission(),
    )?;
    let metadata = synthetic_metadata(width, height, pointer, rotation, region, 1);
    let clean = test_clean(&device, &source, metadata.clone())?;
    let pointer_resource = test_pointer_resource(&device, pointer)?;
    match reducer.submit(
        &clean,
        pointer_resource.as_ref(),
        CaptureExtent::try_new(max_width, u32::MAX).expect("test extent"),
        metadata,
    )? {
        SubmitOutcome::Submitted => {}
        SubmitOutcome::Busy => {
            return Err(GpuReductionError::operation(
                "fresh test reduction unexpectedly found a busy ring",
            ));
        }
    }
    poll_test_reduction(&mut reducer)
}

#[cfg(test)]
pub(super) fn reduce_pointer_sequence(
    bgra: &[u8],
    width: u32,
    height: u32,
    first: &PointerState,
    second: &PointerState,
) -> Result<(Vec<u8>, Vec<u8>), GpuReductionError> {
    let (device, context) = test_device()?;
    let source = test_source(&device, bgra, width, height)?;
    let mut reducer = GpuReducer::new(
        &device,
        &context,
        crate::shared::default_capture_resource_admission(),
    )?;
    let region = CaptureRegion::full(width, height);
    let first_metadata =
        synthetic_metadata(width, height, first, DisplayRotation::Identity, region, 1);
    let mut clean = test_clean(&device, &source, first_metadata.clone())?;
    let first_pointer_resource = test_pointer_resource(&device, first)?;
    reducer.submit(
        &clean,
        first_pointer_resource.as_ref(),
        CaptureExtent::try_new(width, u32::MAX).expect("test extent"),
        first_metadata,
    )?;
    let first = poll_test_reduction(&mut reducer)?;
    let second_metadata =
        synthetic_metadata(width, height, second, DisplayRotation::Identity, region, 2);
    clean.metadata = second_metadata.clone();
    let second_pointer_resource = test_pointer_resource(&device, second)?;
    reducer.submit(
        &clean,
        second_pointer_resource.as_ref(),
        CaptureExtent::try_new(width, u32::MAX).expect("test extent"),
        second_metadata,
    )?;
    let second = poll_test_reduction(&mut reducer)?;
    Ok((first, second))
}

#[cfg(test)]
fn test_pointer_resource(
    device: &ID3D11Device,
    pointer: &PointerState,
) -> Result<Option<super::gpu_surface::PointerResource>, GpuReductionError> {
    if !pointer.visible || pointer.shape.is_none() {
        return Ok(None);
    }
    let admission = crate::shared::default_capture_resource_admission();
    let mut resource = None;
    super::gpu_surface::ensure_pointer_resource(
        device,
        &mut resource,
        pointer,
        u64::MAX,
        admission.as_ref(),
    )
    .map_err(GpuReductionError::capture_resource)?;
    Ok(resource)
}

#[cfg(test)]
pub(super) fn test_device() -> Result<(ID3D11Device, ID3D11DeviceContext), GpuReductionError> {
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0};
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice,
    };
    use windows::Win32::Graphics::Dxgi::IDXGIAdapter;

    let mut device = None;
    let mut context = None;
    // SAFETY: WARP takes no adapter, the feature-level slice is live, and both
    // out-pointers are caller-owned locals.
    unsafe {
        D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            D3D_DRIVER_TYPE_WARP,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .map_err(|error| GpuReductionError::windows("create WARP test device", error))?;
    Ok((
        device.ok_or_else(|| GpuReductionError::operation("WARP returned no device"))?,
        context.ok_or_else(|| GpuReductionError::operation("WARP returned no context"))?,
    ))
}

#[cfg(test)]
pub(super) fn test_source(
    device: &ID3D11Device,
    bgra: &[u8],
    width: u32,
    height: u32,
) -> Result<ID3D11Texture2D, GpuReductionError> {
    let expected_len = checked_rgba_len(width, height, "validate WARP test source")?;
    if bgra.len() != expected_len {
        return Err(GpuReductionError::operation(format!(
            "WARP test source has {} bytes, expected {expected_len}",
            bgra.len()
        )));
    }
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0.cast_unsigned(),
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let initial = D3D11_SUBRESOURCE_DATA {
        pSysMem: bgra.as_ptr().cast(),
        SysMemPitch: checked_rgba_row_pitch(width, height, "create WARP test source")?,
        SysMemSlicePitch: 0,
    };
    create_texture(device, &desc, Some(&initial))
}

#[cfg(test)]
fn test_clean(
    device: &ID3D11Device,
    source: &ID3D11Texture2D,
    metadata: CaptureMetadata,
) -> Result<RetainedDesktop, GpuReductionError> {
    Ok(RetainedDesktop {
        texture: source.clone(),
        srv: create_srv(device, source)?,
        metadata,
        _resource_lease: None,
    })
}

#[cfg(test)]
fn poll_test_reduction(reducer: &mut GpuReducer) -> Result<Vec<u8>, GpuReductionError> {
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut output = Vec::new();
    while Instant::now() < deadline {
        if reducer.poll(&mut output)?.is_some() {
            return Ok(output);
        }
        std::thread::yield_now();
    }
    Err(GpuReductionError::operation(
        "WARP reduction query did not complete within two seconds",
    ))
}
