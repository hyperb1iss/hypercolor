use std::ffi::{CStr, c_void};
use std::mem::{size_of, transmute};
use std::sync::OnceLock;

use thiserror::Error;
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::Graphics::Direct3D::Fxc::{
    D3DCOMPILE_ENABLE_STRICTNESS, D3DCOMPILE_OPTIMIZATION_LEVEL3,
};
use windows::Win32::Graphics::Direct3D::{D3D_SHADER_MACRO, ID3DBlob};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_ASYNC_GETDATA_DONOTFLUSH, D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_SHADER_RESOURCE,
    D3D11_BIND_UNORDERED_ACCESS, D3D11_BUFFER_DESC, D3D11_CPU_ACCESS_READ,
    D3D11_FORMAT_SUPPORT_SHADER_SAMPLE, D3D11_FORMAT_SUPPORT_TYPED_UNORDERED_ACCESS_VIEW,
    D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_QUERY_DESC, D3D11_QUERY_EVENT,
    D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
    ID3D11Buffer, ID3D11ComputeShader, ID3D11Device, ID3D11DeviceContext, ID3D11Query,
    ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11UnorderedAccessView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_UINT, DXGI_FORMAT_R8G8B8A8_UNORM,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{BOOL, HRESULT, Interface, PCSTR, w};

use super::{PointerShapeKind, PointerState};
use crate::{CursorInfo, DisplayRotation, subsample_stride, subsampled_extent};

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
#[error("{0}")]
pub(super) struct GpuReductionError(String);

impl GpuReductionError {
    fn windows(context: &'static str, error: impl std::fmt::Display) -> Self {
        Self(format!("{context}: {error}"))
    }
}

struct ShaderBytecode {
    composite: Vec<u8>,
    reduce: Vec<u8>,
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
                composite: compile_entry(compile, c"composite_cursor")?,
                reduce: compile_entry(compile, c"reduce_desktop")?,
            })
        })
        .as_ref()
        .map_err(|message| GpuReductionError(message.clone()))
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResourceKey {
    width: u32,
    height: u32,
    output_width: u32,
    output_height: u32,
    stride: u32,
    format: DXGI_FORMAT,
}

struct PendingFrame {
    cursor: CursorInfo,
}

struct ReadbackSlot {
    staging: ID3D11Texture2D,
    query: ID3D11Query,
    pending: Option<PendingFrame>,
}

struct Resources {
    key: ResourceKey,
    clean: ID3D11Texture2D,
    clean_srv: ID3D11ShaderResourceView,
    composite: ID3D11Texture2D,
    composite_srv: ID3D11ShaderResourceView,
    composite_uav: ID3D11UnorderedAccessView,
    reduced: ID3D11Texture2D,
    reduced_uav: ID3D11UnorderedAccessView,
    slots: Vec<ReadbackSlot>,
    write_index: usize,
    read_index: usize,
}

struct PointerResource {
    generation: u64,
    width: u32,
    height: u32,
    srv: ID3D11ShaderResourceView,
}

pub(super) enum SubmitOutcome {
    Submitted,
    Busy,
}

pub(super) struct ReducedFrame {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) native_width: u32,
    pub(super) native_height: u32,
    pub(super) cursor: CursorInfo,
    pub(super) bytes: usize,
}

pub(super) struct GpuReducer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    composite_shader: ID3D11ComputeShader,
    reduce_shader: ID3D11ComputeShader,
    params: ID3D11Buffer,
    resources: Option<Resources>,
    pointer: Option<PointerResource>,
}

impl GpuReducer {
    pub(super) fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
    ) -> Result<Self, GpuReductionError> {
        let bytecode = compiled_shaders()?;
        let composite_shader = create_compute_shader(device, &bytecode.composite)?;
        let reduce_shader = create_compute_shader(device, &bytecode.reduce)?;
        let params = create_constant_buffer(device)?;
        Ok(Self {
            device: device.clone(),
            context: context.clone(),
            composite_shader,
            reduce_shader,
            params,
            resources: None,
            pointer: None,
        })
    }

    pub(super) fn has_clean_desktop(&self) -> bool {
        self.resources.is_some()
    }

    pub(super) fn clean_desktop(&self) -> Option<ID3D11Texture2D> {
        self.resources
            .as_ref()
            .map(|resources| resources.clean.clone())
    }

    pub(super) fn submit(
        &mut self,
        texture: Option<&ID3D11Texture2D>,
        max_width: u32,
        pointer: &PointerState,
        rotation: DisplayRotation,
    ) -> Result<SubmitOutcome, GpuReductionError> {
        self.ensure_resources(texture, max_width)?;
        self.ensure_pointer(pointer)?;
        let context = self.context.clone();
        let params_buffer = self.params.clone();

        let resources = self
            .resources
            .as_mut()
            .expect("resource initialization succeeds before submission");
        if let Some(texture) = texture {
            // SAFETY: the clean texture was created from the acquired texture
            // descriptor on the same device.
            unsafe { self.context.CopyResource(&resources.clean, texture) };
        }
        if resources.slots[resources.write_index].pending.is_some() {
            return Ok(SubmitOutcome::Busy);
        }

        // SAFETY: both resources share the source descriptor and device.
        unsafe {
            self.context
                .CopyResource(&resources.composite, &resources.clean)
        };
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
            pointer_height: shape.map_or(0, super::PointerShape::visible_height),
        };
        update_params(&context, &params_buffer, &params);

        if let (Some(shape), Some(pointer_resource)) = (shape, self.pointer.as_ref()) {
            let srvs = [
                Some(resources.clean_srv.clone()),
                Some(pointer_resource.srv.clone()),
            ];
            let uavs = [Some(resources.composite_uav.clone())];
            // SAFETY: all views and the constant buffer belong to this device;
            // dispatch bounds are clipped by the shader.
            unsafe {
                self.context
                    .CSSetConstantBuffers(0, Some(&[Some(self.params.clone())]));
                self.context.CSSetShaderResources(0, Some(&srvs));
                self.context
                    .CSSetUnorderedAccessViews(0, 1, Some(uavs.as_ptr()), None);
                self.context
                    .CSSetShader(&self.composite_shader, None::<&[Option<_>]>);
                self.context.Dispatch(
                    shape.width.div_ceil(THREAD_GROUP),
                    shape.visible_height().div_ceil(THREAD_GROUP),
                    1,
                );
            }
            unbind_compute_views(&context);
        }

        let srvs = [Some(resources.composite_srv.clone()), None];
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
        slot.pending = Some(PendingFrame {
            cursor: pointer.cursor_info(resources.key.width, resources.key.height, rotation),
        });
        resources.write_index = (resources.write_index + 1) % resources.slots.len();
        Ok(SubmitOutcome::Submitted)
    }

    pub(super) fn poll(
        &mut self,
        rgba: &mut Vec<u8>,
    ) -> Result<Option<ReducedFrame>, GpuReductionError> {
        if !self.query_ready()? {
            return Ok(None);
        }
        self.read_ready(rgba).map(Some)
    }

    fn query_ready(&self) -> Result<bool, GpuReductionError> {
        let Some(resources) = self.resources.as_ref() else {
            return Ok(false);
        };
        let slot = &resources.slots[resources.read_index];
        if slot.pending.is_none() {
            return Ok(false);
        }
        let mut ready = BOOL::default();
        // SAFETY: the query is live, and `ready` supplies the documented BOOL
        // output storage. DONOTFLUSH keeps polling non-blocking.
        unsafe {
            self.context.GetData(
                &slot.query,
                Some((&raw mut ready).cast()),
                u32::try_from(size_of::<BOOL>()).unwrap_or(u32::MAX),
                D3D11_ASYNC_GETDATA_DONOTFLUSH.0.cast_unsigned(),
            )
        }
        .map_err(|error| GpuReductionError::windows("poll reduction query", error))?;
        Ok(ready.as_bool())
    }

    fn read_ready(&mut self, rgba: &mut Vec<u8>) -> Result<ReducedFrame, GpuReductionError> {
        let resources = self
            .resources
            .as_mut()
            .expect("a ready query belongs to initialized resources");
        let slot = &mut resources.slots[resources.read_index];
        let pending = slot
            .pending
            .as_ref()
            .expect("a ready query has pending frame metadata");
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: staging is CPU-readable and its event query completed.
        unsafe {
            self.context
                .Map(&slot.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(|error| GpuReductionError::windows("map reduced staging texture", error))?;
        let row_bytes = resources.key.output_width as usize * 4;
        let output_len = row_bytes * resources.key.output_height as usize;
        rgba.resize(output_len, 0);
        let row_pitch = mapped.RowPitch as usize;
        // SAFETY: Map exposes RowPitch bytes for each output row until Unmap.
        let source = unsafe {
            std::slice::from_raw_parts(
                mapped.pData.cast::<u8>(),
                row_pitch * resources.key.output_height as usize,
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

        let cursor = pending.cursor;
        slot.pending = None;
        resources.read_index = (resources.read_index + 1) % resources.slots.len();
        Ok(ReducedFrame {
            width: resources.key.output_width,
            height: resources.key.output_height,
            native_width: resources.key.width,
            native_height: resources.key.height,
            cursor,
            bytes: output_len,
        })
    }

    fn ensure_resources(
        &mut self,
        texture: Option<&ID3D11Texture2D>,
        max_width: u32,
    ) -> Result<(), GpuReductionError> {
        let source_desc = if let Some(texture) = texture {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            // SAFETY: GetDesc fills a live caller-owned descriptor.
            unsafe { texture.GetDesc(&mut desc) };
            desc
        } else {
            let Some(resources) = self.resources.as_ref() else {
                return Err(GpuReductionError(
                    "no retained clean desktop is available for reduction".to_owned(),
                ));
            };
            source_desc(resources.key)
        };
        let key = resource_key(&source_desc, max_width)?;
        if self
            .resources
            .as_ref()
            .is_some_and(|resources| resources.key == key)
        {
            return Ok(());
        }

        let mut replacement = create_resources(&self.device, key)?;
        if texture.is_none()
            && let Some(previous) = self.resources.as_ref()
            && previous.key.width == key.width
            && previous.key.height == key.height
            && previous.key.format == key.format
        {
            // SAFETY: source descriptors match and both resources belong to
            // this device. This preserves static desktops across grid changes.
            unsafe {
                self.context
                    .CopyResource(&replacement.clean, &previous.clean)
            };
        }
        replacement.write_index = 0;
        replacement.read_index = 0;
        self.resources = Some(replacement);
        Ok(())
    }

    fn ensure_pointer(&mut self, pointer: &PointerState) -> Result<(), GpuReductionError> {
        let Some(shape) = pointer.shape.as_ref() else {
            return Ok(());
        };
        let visible_height = shape.visible_height();
        if self.pointer.as_ref().is_some_and(|resource| {
            resource.generation == pointer.shape_generation
                && resource.width == shape.width
                && resource.height == visible_height
        }) {
            return Ok(());
        }

        let pixels = normalized_pointer(shape);
        let desc = D3D11_TEXTURE2D_DESC {
            Width: shape.width,
            Height: visible_height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UINT,
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
            pSysMem: pixels.as_ptr().cast(),
            SysMemPitch: shape.width.saturating_mul(4),
            SysMemSlicePitch: 0,
        };
        let texture = create_texture(&self.device, &desc, Some(&initial))?;
        let srv = create_srv(&self.device, &texture)?;
        self.pointer = Some(PointerResource {
            generation: pointer.shape_generation,
            width: shape.width,
            height: visible_height,
            srv,
        });
        Ok(())
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
    max_width: u32,
) -> Result<ResourceKey, GpuReductionError> {
    if desc.Width == 0 || desc.Height == 0 {
        return Err(GpuReductionError(
            "duplicated texture has an empty extent".to_owned(),
        ));
    }
    let stride = subsample_stride(desc.Width, max_width);
    Ok(ResourceKey {
        width: desc.Width,
        height: desc.Height,
        output_width: subsampled_extent(desc.Width, stride),
        output_height: subsampled_extent(desc.Height, stride),
        stride,
        format: desc.Format,
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
) -> Result<Resources, GpuReductionError> {
    require_format_support(device, key.format)?;
    require_format_support(device, DXGI_FORMAT_R8G8B8A8_UNORM)?;
    let source = source_desc(key);
    let clean_desc = D3D11_TEXTURE2D_DESC {
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0.cast_unsigned(),
        ..source
    };
    let clean = create_texture(device, &clean_desc, None)?;
    let clean_srv = create_srv(device, &clean)?;
    let composite_desc = D3D11_TEXTURE2D_DESC {
        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_UNORDERED_ACCESS.0).cast_unsigned(),
        ..source
    };
    let composite = create_texture(device, &composite_desc, None)?;
    let composite_srv = create_srv(device, &composite)?;
    let composite_uav = create_uav(device, &composite)?;

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
    let mut slots = Vec::with_capacity(READBACK_RING_LEN);
    for _ in 0..READBACK_RING_LEN {
        slots.push(ReadbackSlot {
            staging: create_texture(device, &staging_desc, None)?,
            query: create_event_query(device)?,
            pending: None,
        });
    }
    Ok(Resources {
        key,
        clean,
        clean_srv,
        composite,
        composite_srv,
        composite_uav,
        reduced,
        reduced_uav,
        slots,
        write_index: 0,
        read_index: 0,
    })
}

fn require_format_support(
    device: &ID3D11Device,
    format: DXGI_FORMAT,
) -> Result<(), GpuReductionError> {
    // SAFETY: format support is a read-only device query.
    let support = unsafe { device.CheckFormatSupport(format) }
        .map_err(|error| GpuReductionError::windows("query texture format support", error))?;
    let required = (D3D11_FORMAT_SUPPORT_SHADER_SAMPLE.0
        | D3D11_FORMAT_SUPPORT_TYPED_UNORDERED_ACCESS_VIEW.0)
        .cast_unsigned();
    if support & required != required {
        return Err(GpuReductionError(format!(
            "format {:?} lacks shader-sample or typed-UAV support",
            format
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
    unsafe { device.CreateComputeShader(bytecode, None, Some(&mut shader)) }
        .map_err(|error| GpuReductionError::windows("create capture compute shader", error))?;
    shader.ok_or_else(|| GpuReductionError("compute shader creation returned no shader".to_owned()))
}

fn create_constant_buffer(device: &ID3D11Device) -> Result<ID3D11Buffer, GpuReductionError> {
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
    unsafe { device.CreateBuffer(&desc, None, Some(&mut buffer)) }
        .map_err(|error| GpuReductionError::windows("create reduction constants", error))?;
    buffer
        .ok_or_else(|| GpuReductionError("constant buffer creation returned no buffer".to_owned()))
}

fn create_texture(
    device: &ID3D11Device,
    desc: &D3D11_TEXTURE2D_DESC,
    initial: Option<&D3D11_SUBRESOURCE_DATA>,
) -> Result<ID3D11Texture2D, GpuReductionError> {
    let mut texture = None;
    // SAFETY: descriptor and optional initial data are live through the call;
    // the out-pointer remains valid.
    unsafe { device.CreateTexture2D(desc, initial.map(std::ptr::from_ref), Some(&mut texture)) }
        .map_err(|error| GpuReductionError::windows("create reduction texture", error))?;
    texture.ok_or_else(|| GpuReductionError("texture creation returned no texture".to_owned()))
}

fn create_srv(
    device: &ID3D11Device,
    texture: &ID3D11Texture2D,
) -> Result<ID3D11ShaderResourceView, GpuReductionError> {
    let mut view = None;
    // SAFETY: the texture supports shader-resource binding and the default
    // view spans its only subresource.
    unsafe { device.CreateShaderResourceView(texture, None, Some(&mut view)) }
        .map_err(|error| GpuReductionError::windows("create reduction SRV", error))?;
    view.ok_or_else(|| GpuReductionError("SRV creation returned no view".to_owned()))
}

fn create_uav(
    device: &ID3D11Device,
    texture: &ID3D11Texture2D,
) -> Result<ID3D11UnorderedAccessView, GpuReductionError> {
    let mut view = None;
    // SAFETY: the texture format was checked for typed UAV support and the
    // default view spans its only subresource.
    unsafe { device.CreateUnorderedAccessView(texture, None, Some(&mut view)) }
        .map_err(|error| GpuReductionError::windows("create reduction UAV", error))?;
    view.ok_or_else(|| GpuReductionError("UAV creation returned no view".to_owned()))
}

fn create_event_query(device: &ID3D11Device) -> Result<ID3D11Query, GpuReductionError> {
    let desc = D3D11_QUERY_DESC {
        Query: D3D11_QUERY_EVENT,
        MiscFlags: 0,
    };
    let mut query = None;
    // SAFETY: the event query descriptor and out-pointer remain live.
    unsafe { device.CreateQuery(&desc, Some(&mut query)) }
        .map_err(|error| GpuReductionError::windows("create reduction event query", error))?;
    query.ok_or_else(|| GpuReductionError("query creation returned no query".to_owned()))
}

fn normalized_pointer(shape: &super::PointerShape) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(shape.width as usize * shape.visible_height() as usize * 4);
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
    pixels
}

const fn pointer_kind_code(kind: PointerShapeKind) -> u32 {
    match kind {
        PointerShapeKind::Color => 0,
        PointerShapeKind::MaskedColor => 1,
        PointerShapeKind::Monochrome => 2,
    }
}

const fn rotation_code(rotation: DisplayRotation) -> u32 {
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
    pub acquisition: std::time::Duration,
    /// CPU time to enqueue cursor composition, reduction, and staging copy.
    pub gpu_reduction: std::time::Duration,
    /// Time until the event query reported the reduced staging slot ready.
    pub wait: std::time::Duration,
    /// Time to map and copy the tightly packed reduced plane.
    pub map: std::time::Duration,
    /// Native source bytes represented by the acquisition.
    pub source_bytes: u64,
    /// Bytes actually read back from the reduced surface.
    pub readback_bytes: u64,
}

/// Stable-resource D3D11 harness for the Windows capture Criterion target.
#[cfg(feature = "capture-bench")]
pub struct CaptureReductionBenchmark {
    context: ID3D11DeviceContext,
    reducer: GpuReducer,
    source: ID3D11Texture2D,
    pointer: PointerState,
    max_width: u32,
    source_bytes: u64,
    output: Vec<u8>,
}

#[cfg(feature = "capture-bench")]
impl CaptureReductionBenchmark {
    /// Create a hardware-backed stable-resource reduction fixture.
    pub fn new(width: u32, height: u32, max_width: u32) -> Result<Self, String> {
        use windows::Win32::Graphics::Direct3D::{
            D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0,
        };
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice,
        };
        use windows::Win32::Graphics::Dxgi::IDXGIAdapter;

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
        .map_err(|error| format!("create benchmark D3D11 device: {error}"))?;
        let device = device.ok_or_else(|| "D3D11 returned no benchmark device".to_owned())?;
        let context = context.ok_or_else(|| "D3D11 returned no benchmark context".to_owned())?;
        let pixel_count = u64::from(width) * u64::from(height);
        let rgba = (0..pixel_count)
            .flat_map(|index| {
                let value = index as u8;
                [value, value.wrapping_add(47), value.wrapping_add(109), 0xFF]
            })
            .collect::<Vec<_>>();
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
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
            SysMemPitch: width.saturating_mul(4),
            SysMemSlicePitch: 0,
        };
        let source = create_texture(&device, &desc, Some(&initial)).map_err(|error| error.0)?;
        let mut reducer = GpuReducer::new(&device, &context).map_err(|error| error.0)?;
        reducer
            .ensure_resources(Some(&source), max_width)
            .map_err(|error| error.0)?;
        Ok(Self {
            context,
            reducer,
            source,
            pointer: PointerState::default(),
            max_width,
            source_bytes: pixel_count.saturating_mul(4),
            output: Vec::new(),
        })
    }

    /// Run one acquisition-copy, reduction, wait, and reduced-map sample.
    pub fn sample(&mut self) -> Result<ReductionBenchmarkSample, String> {
        use std::time::Instant;

        let acquisition_started = Instant::now();
        let clean = self
            .reducer
            .resources
            .as_ref()
            .expect("benchmark resources are initialized")
            .clean
            .clone();
        // SAFETY: the source and clean textures have identical descriptors and
        // belong to the same device.
        unsafe { self.context.CopyResource(&clean, &self.source) };
        let acquisition = acquisition_started.elapsed();

        let reduction_started = Instant::now();
        match self
            .reducer
            .submit(
                None,
                self.max_width,
                &self.pointer,
                DisplayRotation::Identity,
            )
            .map_err(|error| error.0)?
        {
            SubmitOutcome::Submitted => {}
            SubmitOutcome::Busy => return Err("benchmark readback ring is busy".to_owned()),
        }
        let gpu_reduction = reduction_started.elapsed();
        // SAFETY: flushing the immediate benchmark context has no preconditions.
        unsafe { self.context.Flush() };

        let wait_started = Instant::now();
        while !self.reducer.query_ready().map_err(|error| error.0)? {
            std::hint::spin_loop();
        }
        let wait = wait_started.elapsed();
        let map_started = Instant::now();
        let frame = self
            .reducer
            .read_ready(&mut self.output)
            .map_err(|error| error.0)?;
        let map = map_started.elapsed();
        Ok(ReductionBenchmarkSample {
            acquisition,
            gpu_reduction,
            wait,
            map,
            source_bytes: self.source_bytes,
            readback_bytes: frame.bytes as u64,
        })
    }
}

#[cfg(test)]
pub(super) fn compile_shaders_for_test() -> Result<(), GpuReductionError> {
    compiled_shaders().map(|_| ())
}

#[cfg(test)]
pub(super) fn normalized_pointer_for_test(shape: &super::PointerShape) -> Vec<u8> {
    normalized_pointer(shape)
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
    let mut reducer = GpuReducer::new(&device, &context)?;
    for submission in 0..=READBACK_RING_LEN {
        let outcome = reducer.submit(
            (submission == 0).then_some(&source),
            1,
            &pointer,
            DisplayRotation::Identity,
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
pub(super) fn reduce_fixture(
    bgra: &[u8],
    width: u32,
    height: u32,
    max_width: u32,
    pointer: &PointerState,
    rotation: DisplayRotation,
) -> Result<Vec<u8>, GpuReductionError> {
    let (device, context) = test_device()?;
    let source = test_source(&device, bgra, width, height)?;
    let mut reducer = GpuReducer::new(&device, &context)?;
    match reducer.submit(Some(&source), max_width, pointer, rotation)? {
        SubmitOutcome::Submitted => {}
        SubmitOutcome::Busy => {
            return Err(GpuReductionError(
                "fresh test reduction unexpectedly found a busy ring".to_owned(),
            ));
        }
    }
    poll_test_reduction(&mut reducer, &context)
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
    let mut reducer = GpuReducer::new(&device, &context)?;
    reducer.submit(Some(&source), width, first, DisplayRotation::Identity)?;
    let first = poll_test_reduction(&mut reducer, &context)?;
    reducer.submit(None, width, second, DisplayRotation::Identity)?;
    let second = poll_test_reduction(&mut reducer, &context)?;
    Ok((first, second))
}

#[cfg(test)]
fn test_device() -> Result<(ID3D11Device, ID3D11DeviceContext), GpuReductionError> {
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
        device.ok_or_else(|| GpuReductionError("WARP returned no device".to_owned()))?,
        context.ok_or_else(|| GpuReductionError("WARP returned no context".to_owned()))?,
    ))
}

#[cfg(test)]
fn test_source(
    device: &ID3D11Device,
    bgra: &[u8],
    width: u32,
    height: u32,
) -> Result<ID3D11Texture2D, GpuReductionError> {
    let rgba_source = bgra
        .chunks_exact(4)
        .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], pixel[3]])
        .collect::<Vec<_>>();
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
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
        pSysMem: rgba_source.as_ptr().cast(),
        SysMemPitch: width.saturating_mul(4),
        SysMemSlicePitch: 0,
    };
    create_texture(device, &desc, Some(&initial))
}

#[cfg(test)]
fn poll_test_reduction(
    reducer: &mut GpuReducer,
    context: &ID3D11DeviceContext,
) -> Result<Vec<u8>, GpuReductionError> {
    use std::time::{Duration, Instant};

    // SAFETY: flushing the immediate test context has no preconditions.
    unsafe { context.Flush() };
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut output = Vec::new();
    while Instant::now() < deadline {
        if reducer.poll(&mut output)?.is_some() {
            return Ok(output);
        }
        std::thread::yield_now();
    }
    Err(GpuReductionError(
        "WARP reduction query did not complete within two seconds".to_owned(),
    ))
}
