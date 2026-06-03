use std::ffi::{CStr, c_void};
use std::num::NonZeroU32;
use std::time::Instant;

use glow::HasContext;

use super::fence::{create_gl_fence, delete_gl_fence, wait_for_gl_blit_completion};
use super::loader::{lookup_process_gl_symbol, process_gl_loader_available};
use super::{
    GlFramebufferSource, GlFramebufferStateSnapshot, ImportedFrameTimings,
    LinuxGlFramebufferImportDescriptor, LinuxGpuInteropError, Result, elapsed_micros,
};

const GL_DEDICATED_MEMORY_OBJECT_EXT: u32 = 0x9581;
const GL_HANDLE_TYPE_OPAQUE_FD_EXT: u32 = 0x9586;

type GlCreateMemoryObjectsExt = unsafe extern "system" fn(i32, *mut u32);
type GlMemoryObjectParameterivExt = unsafe extern "system" fn(u32, u32, *const i32);
type GlImportMemoryFdExt = unsafe extern "system" fn(u32, u64, u32, i32);
type GlTexStorageMem2DExt = unsafe extern "system" fn(u32, i32, u32, i32, i32, u32, u64);
type GlDeleteMemoryObjectsExt = unsafe extern "system" fn(i32, *const u32);

/// Loaded GL entry points for `GL_EXT_memory_object_fd`.
#[derive(Clone, Copy)]
pub struct GlExternalMemoryFunctions {
    /// `glCreateMemoryObjectsEXT`
    pub create_memory_objects_ext: GlCreateMemoryObjectsExt,
    /// `glMemoryObjectParameterivEXT`
    pub memory_object_parameteriv_ext: GlMemoryObjectParameterivExt,
    /// `glImportMemoryFdEXT`
    pub import_memory_fd_ext: GlImportMemoryFdExt,
    /// `glTexStorageMem2DEXT`
    pub tex_storage_mem_2d_ext: GlTexStorageMem2DExt,
    /// `glDeleteMemoryObjectsEXT`
    pub delete_memory_objects_ext: GlDeleteMemoryObjectsExt,
}

impl GlExternalMemoryFunctions {
    /// Loads required entry points from a current GL context.
    ///
    /// The callback should return the address for the supplied symbol name, or
    /// a null pointer when the symbol is unavailable.
    pub fn load_from(mut get_proc_address: impl FnMut(&CStr) -> *const c_void) -> Result<Self> {
        let create_memory_objects_ext = get_required_proc_address(
            c"glCreateMemoryObjectsEXT",
            "glCreateMemoryObjectsEXT",
            &mut get_proc_address,
        )?;
        let memory_object_parameteriv_ext = get_required_proc_address(
            c"glMemoryObjectParameterivEXT",
            "glMemoryObjectParameterivEXT",
            &mut get_proc_address,
        )?;
        let import_memory_fd_ext = get_required_proc_address(
            c"glImportMemoryFdEXT",
            "glImportMemoryFdEXT",
            &mut get_proc_address,
        )?;
        let tex_storage_mem_2d_ext = get_required_proc_address(
            c"glTexStorageMem2DEXT",
            "glTexStorageMem2DEXT",
            &mut get_proc_address,
        )?;
        let delete_memory_objects_ext = get_required_proc_address(
            c"glDeleteMemoryObjectsEXT",
            "glDeleteMemoryObjectsEXT",
            &mut get_proc_address,
        )?;

        Ok(Self {
            // SAFETY: the symbol is loaded from the current GL context using
            // the exact ABI and signature specified by GL_EXT_memory_object.
            create_memory_objects_ext: unsafe {
                std::mem::transmute::<*const c_void, GlCreateMemoryObjectsExt>(
                    create_memory_objects_ext,
                )
            },
            // SAFETY: the symbol is loaded from the current GL context using
            // the exact ABI and signature specified by GL_EXT_memory_object.
            memory_object_parameteriv_ext: unsafe {
                std::mem::transmute::<*const c_void, GlMemoryObjectParameterivExt>(
                    memory_object_parameteriv_ext,
                )
            },
            // SAFETY: the symbol is loaded from the current GL context using
            // the exact ABI and signature specified by GL_EXT_memory_object_fd.
            import_memory_fd_ext: unsafe {
                std::mem::transmute::<*const c_void, GlImportMemoryFdExt>(import_memory_fd_ext)
            },
            // SAFETY: the symbol is loaded from the current GL context using
            // the exact ABI and signature specified by GL_EXT_memory_object.
            tex_storage_mem_2d_ext: unsafe {
                std::mem::transmute::<*const c_void, GlTexStorageMem2DExt>(tex_storage_mem_2d_ext)
            },
            // SAFETY: the symbol is loaded from the current GL context using
            // the exact ABI and signature specified by GL_EXT_memory_object.
            delete_memory_objects_ext: unsafe {
                std::mem::transmute::<*const c_void, GlDeleteMemoryObjectsExt>(
                    delete_memory_objects_ext,
                )
            },
        })
    }

    /// Loads required entry points from libGL/libEGL process loaders.
    pub fn load_from_process() -> Result<Self> {
        if !process_gl_loader_available() {
            return Err(LinuxGpuInteropError::GlProcLoaderUnavailable);
        }
        Self::load_from(lookup_process_gl_symbol)
    }
}

pub(super) struct GlImportedImageBinding {
    memory_object: u32,
    texture: Option<glow::NativeTexture>,
    framebuffer: Option<glow::NativeFramebuffer>,
}

pub(super) struct PendingGlBlit {
    pub(super) fence: glow::NativeFence,
    pub(super) timings: ImportedFrameTimings,
}

impl GlImportedImageBinding {
    pub(super) fn create(
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
        descriptor: LinuxGlFramebufferImportDescriptor,
        memory_fd: i32,
        allocation_size: u64,
    ) -> Result<Self> {
        let bindings = capture_gl_bindings(gl);
        clear_gl_errors(gl);
        let mut memory_object = 0;
        let mut texture = None;
        let mut framebuffer = None;
        let result = (|| {
            // SAFETY: the function pointer was loaded from the current GL context,
            // and memory_object points to valid writable storage for one object.
            unsafe { (gl_external_memory.create_memory_objects_ext)(1, &mut memory_object) };
            check_gl_error(gl, "glCreateMemoryObjectsEXT")?;

            let dedicated = i32::from(glow::TRUE);
            // SAFETY: memory_object was created by GL above, and dedicated
            // points to stable storage for the duration of this call.
            unsafe {
                (gl_external_memory.memory_object_parameteriv_ext)(
                    memory_object,
                    GL_DEDICATED_MEMORY_OBJECT_EXT,
                    &dedicated,
                );
            }
            check_gl_error(gl, "glMemoryObjectParameterivEXT")?;

            let gl_fd = duplicate_fd(memory_fd)?;
            // SAFETY: gl_fd is a duplicate of the Vulkan memory FD; GL consumes
            // the duplicate while Rust keeps ownership of the original FD.
            unsafe {
                (gl_external_memory.import_memory_fd_ext)(
                    memory_object,
                    allocation_size,
                    GL_HANDLE_TYPE_OPAQUE_FD_EXT,
                    gl_fd,
                );
            }
            check_gl_error(gl, "glImportMemoryFdEXT")?;

            // SAFETY: a current GL context is required by the public import API.
            let imported_texture = unsafe { gl.create_texture() }.map_err(|message| {
                LinuxGpuInteropError::GlCreateResource {
                    resource: "texture",
                    message,
                }
            })?;
            texture = Some(imported_texture);

            // SAFETY: imported_texture belongs to this context and is valid until
            // cleanup at the end of this function.
            unsafe { gl.bind_texture(glow::TEXTURE_2D, texture) };
            // SAFETY: memory_object names external memory imported above, and the
            // texture bound to TEXTURE_2D receives storage from that memory.
            unsafe {
                (gl_external_memory.tex_storage_mem_2d_ext)(
                    glow::TEXTURE_2D,
                    1,
                    descriptor.format.gl_internal_format(),
                    descriptor.width_i32(),
                    descriptor.height_i32(),
                    memory_object,
                    0,
                );
            }
            check_gl_error(gl, "glTexStorageMem2DEXT")?;

            // SAFETY: a current GL context is required by the public import API.
            let draw_framebuffer = unsafe { gl.create_framebuffer() }.map_err(|message| {
                LinuxGpuInteropError::GlCreateResource {
                    resource: "framebuffer",
                    message,
                }
            })?;
            framebuffer = Some(draw_framebuffer);

            // SAFETY: framebuffer and texture are valid GL objects owned by this
            // context for the duration of the blit.
            unsafe {
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, framebuffer);
                gl.framebuffer_texture_2d(
                    glow::DRAW_FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    texture,
                    0,
                );
                gl.draw_buffer(glow::COLOR_ATTACHMENT0);
            }
            // SAFETY: DRAW_FRAMEBUFFER is bound above.
            let framebuffer_status = unsafe { gl.check_framebuffer_status(glow::DRAW_FRAMEBUFFER) };
            if framebuffer_status != glow::FRAMEBUFFER_COMPLETE {
                return Err(LinuxGpuInteropError::GlFramebufferIncomplete {
                    status: framebuffer_status,
                });
            }

            Ok(Self {
                memory_object,
                texture,
                framebuffer,
            })
        })();

        let result = match result {
            Ok(binding) => Ok(binding),
            Err(error) => {
                cleanup_gl_import_resources(
                    gl,
                    gl_external_memory,
                    framebuffer,
                    texture,
                    memory_object,
                );
                Err(error)
            }
        };
        restore_gl_bindings(gl, bindings);
        result
    }

    pub(super) fn blit_from_framebuffer(
        &self,
        gl: &glow::Context,
        _gl_external_memory: GlExternalMemoryFunctions,
        source_framebuffer: GlFramebufferSource,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Result<ImportedFrameTimings> {
        let bindings = capture_gl_bindings(gl);
        clear_gl_errors(gl);
        let result = (|| {
            let blit_start = Instant::now();
            bind_source_framebuffer_for_blit(gl, source_framebuffer);
            check_gl_error(gl, "glBindFramebuffer(read)")?;
            // SAFETY: the destination framebuffer belongs to this context and
            // is complete and backed by external memory.
            unsafe {
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, self.framebuffer);
            }
            check_gl_error(gl, "glBindFramebuffer(draw)")?;
            prepare_framebuffer_buffers_for_blit(gl);
            check_gl_error(gl, "glReadBuffer/glDrawBuffer")?;
            // SAFETY: the source and destination framebuffer state has been
            // selected and normalized in this current GL context.
            unsafe {
                gl.blit_framebuffer(
                    0,
                    0,
                    descriptor.width_i32(),
                    descriptor.height_i32(),
                    0,
                    descriptor.height_i32(),
                    descriptor.width_i32(),
                    0,
                    glow::COLOR_BUFFER_BIT,
                    glow::NEAREST,
                );
            }
            let blit_us = elapsed_micros(blit_start);
            check_gl_error(gl, "glBlitFramebuffer")?;

            let sync_us = wait_for_gl_blit_completion(gl)?;

            Ok(ImportedFrameTimings {
                blit_us,
                sync_us,
                total_us: 0,
            })
        })();

        restore_gl_bindings(gl, bindings);
        result
    }

    pub(super) fn blit_from_framebuffer_pipelined(
        &self,
        gl: &glow::Context,
        _gl_external_memory: GlExternalMemoryFunctions,
        source_framebuffer: GlFramebufferSource,
        descriptor: LinuxGlFramebufferImportDescriptor,
    ) -> Result<PendingGlBlit> {
        let bindings = capture_gl_bindings(gl);
        clear_gl_errors(gl);
        let result = (|| {
            let blit_start = Instant::now();
            bind_source_framebuffer_for_blit(gl, source_framebuffer);
            check_gl_error(gl, "glBindFramebuffer(read)")?;
            // SAFETY: the destination framebuffer belongs to this context and
            // is complete and backed by external memory.
            unsafe {
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, self.framebuffer);
            }
            check_gl_error(gl, "glBindFramebuffer(draw)")?;
            prepare_framebuffer_buffers_for_blit(gl);
            check_gl_error(gl, "glReadBuffer/glDrawBuffer")?;
            // SAFETY: the source and destination framebuffer state has been
            // selected and normalized in this current GL context.
            unsafe {
                gl.blit_framebuffer(
                    0,
                    0,
                    descriptor.width_i32(),
                    descriptor.height_i32(),
                    0,
                    descriptor.height_i32(),
                    descriptor.width_i32(),
                    0,
                    glow::COLOR_BUFFER_BIT,
                    glow::NEAREST,
                );
            }
            let blit_us = elapsed_micros(blit_start);
            check_gl_error(gl, "glBlitFramebuffer")?;

            let sync_start = Instant::now();
            let fence = create_gl_fence(gl)?;
            // SAFETY: the fence was inserted after the blit on this current GL
            // context; flushing lets later non-blocking polls observe progress.
            unsafe {
                gl.flush();
            }
            if let Err(error) = check_gl_error(gl, "glFlush") {
                delete_gl_fence(gl, fence);
                return Err(error);
            }
            let sync_us = elapsed_micros(sync_start);

            Ok(PendingGlBlit {
                fence,
                timings: ImportedFrameTimings {
                    blit_us,
                    sync_us,
                    total_us: 0,
                },
            })
        })();

        restore_gl_bindings(gl, bindings);
        result
    }

    pub(super) fn framebuffer_state_for_blit(
        &self,
        gl: &glow::Context,
        source_framebuffer: GlFramebufferSource,
    ) -> GlFramebufferStateSnapshot {
        let bindings = capture_gl_bindings(gl);
        clear_gl_errors(gl);
        bind_source_framebuffer_for_blit(gl, source_framebuffer);
        // SAFETY: the bindings are restored before returning. The caller owns
        // the current context and this mirrors the import blit's bind sequence.
        unsafe {
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, self.framebuffer);
            prepare_framebuffer_buffers_for_blit(gl);
        }
        let state = current_gl_framebuffer_state(gl);
        restore_gl_bindings(gl, bindings);
        clear_gl_errors(gl);
        state
    }

    pub(super) fn destroy(
        &mut self,
        gl: &glow::Context,
        gl_external_memory: GlExternalMemoryFunctions,
    ) {
        cleanup_gl_import_resources(
            gl,
            gl_external_memory,
            self.framebuffer.take(),
            self.texture.take(),
            std::mem::take(&mut self.memory_object),
        );
    }
}

fn get_required_proc_address(
    symbol: &'static CStr,
    name: &'static str,
    get_proc_address: &mut impl FnMut(&CStr) -> *const c_void,
) -> Result<*const c_void> {
    let ptr = get_proc_address(symbol);
    if ptr.is_null() {
        Err(LinuxGpuInteropError::MissingGlFunction(name))
    } else {
        Ok(ptr)
    }
}

fn duplicate_fd(fd: i32) -> Result<i32> {
    // SAFETY: dup does not take ownership of fd and returns a new descriptor or
    // -1 with errno set.
    let duplicate = unsafe { libc::dup(fd) };
    if duplicate >= 0 {
        Ok(duplicate)
    } else {
        Err(LinuxGpuInteropError::DuplicateFdFailed {
            errno: std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or_default(),
        })
    }
}

fn cleanup_gl_import_resources(
    gl: &glow::Context,
    gl_external_memory: GlExternalMemoryFunctions,
    framebuffer: Option<glow::NativeFramebuffer>,
    texture: Option<glow::NativeTexture>,
    memory_object: u32,
) {
    // SAFETY: the objects were created in this context when present. Deleting
    // zero memory objects is skipped because zero is not a valid object name.
    unsafe {
        if let Some(framebuffer) = framebuffer {
            gl.delete_framebuffer(framebuffer);
        }
        if let Some(texture) = texture {
            gl.delete_texture(texture);
        }
        if memory_object != 0 {
            (gl_external_memory.delete_memory_objects_ext)(1, &memory_object);
        }
    }
}

#[derive(Clone, Copy)]
struct GlBindingSnapshot {
    read_framebuffer: Option<glow::NativeFramebuffer>,
    draw_framebuffer: Option<glow::NativeFramebuffer>,
    read_buffer: Option<u32>,
    draw_buffer0: Option<u32>,
    texture_2d: Option<glow::NativeTexture>,
}

fn capture_gl_bindings(gl: &glow::Context) -> GlBindingSnapshot {
    // SAFETY: these queries read binding state from the current GL context.
    let read_framebuffer =
        unsafe { framebuffer_from_binding(gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING)) };
    // SAFETY: these queries read binding state from the current GL context.
    let draw_framebuffer =
        unsafe { framebuffer_from_binding(gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING)) };
    // SAFETY: these queries read buffer-selector state from the current GL context.
    let read_buffer = unsafe { enum_from_binding(gl.get_parameter_i32(glow::READ_BUFFER)) };
    // SAFETY: these queries read buffer-selector state from the current GL context.
    let draw_buffer0 = unsafe { enum_from_binding(gl.get_parameter_i32(glow::DRAW_BUFFER0)) };
    // SAFETY: these queries read binding state from the current GL context.
    let texture_2d =
        unsafe { texture_from_binding(gl.get_parameter_i32(glow::TEXTURE_BINDING_2D)) };

    GlBindingSnapshot {
        read_framebuffer,
        draw_framebuffer,
        read_buffer,
        draw_buffer0,
        texture_2d,
    }
}

fn restore_gl_bindings(gl: &glow::Context, bindings: GlBindingSnapshot) {
    // SAFETY: the captured object names came from this same current GL context.
    unsafe {
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, bindings.read_framebuffer);
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, bindings.draw_framebuffer);
        if let Some(read_buffer) = bindings.read_buffer {
            gl.read_buffer(read_buffer);
        }
        if let Some(draw_buffer0) = bindings.draw_buffer0 {
            gl.draw_buffer(draw_buffer0);
        }
        gl.bind_texture(glow::TEXTURE_2D, bindings.texture_2d);
    }
}

fn bind_source_framebuffer_for_blit(gl: &glow::Context, source: GlFramebufferSource) {
    match source {
        GlFramebufferSource::CurrentRead => {}
        GlFramebufferSource::Framebuffer(source) => {
            // SAFETY: the caller supplied a framebuffer for this current GL context.
            unsafe {
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, source);
            }
        }
    }
}

fn prepare_framebuffer_buffers_for_blit(gl: &glow::Context) {
    // SAFETY: the import path binds the destination draw FBO before selecting
    // color buffers. Non-default source FBOs use COLOR_ATTACHMENT0; default
    // source FBOs keep their existing read selector because COLOR_ATTACHMENT0
    // is not a valid default-framebuffer read buffer.
    unsafe {
        if gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING) != 0 {
            gl.read_buffer(glow::COLOR_ATTACHMENT0);
        }
        gl.draw_buffer(glow::COLOR_ATTACHMENT0);
    }
}

pub(super) fn current_gl_framebuffer_state(gl: &glow::Context) -> GlFramebufferStateSnapshot {
    let mut viewport = [0; 4];
    // SAFETY: these queries operate on the current GL context and leave
    // framebuffer bindings unchanged.
    unsafe {
        gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
        GlFramebufferStateSnapshot {
            read_framebuffer: gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING),
            draw_framebuffer: gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING),
            read_buffer: gl.get_parameter_i32(glow::READ_BUFFER),
            draw_buffer0: gl.get_parameter_i32(glow::DRAW_BUFFER0),
            read_status: gl.check_framebuffer_status(glow::READ_FRAMEBUFFER),
            draw_status: gl.check_framebuffer_status(glow::DRAW_FRAMEBUFFER),
            viewport,
        }
    }
}

fn framebuffer_from_binding(binding: i32) -> Option<glow::NativeFramebuffer> {
    u32::try_from(binding)
        .ok()
        .and_then(NonZeroU32::new)
        .map(glow::NativeFramebuffer)
}

fn enum_from_binding(binding: i32) -> Option<u32> {
    u32::try_from(binding).ok()
}

fn texture_from_binding(binding: i32) -> Option<glow::NativeTexture> {
    u32::try_from(binding)
        .ok()
        .and_then(NonZeroU32::new)
        .map(glow::NativeTexture)
}

pub(super) fn clear_gl_errors(gl: &glow::Context) {
    for _ in 0..16 {
        // SAFETY: this reads and clears the current GL error flag.
        if unsafe { gl.get_error() } == glow::NO_ERROR {
            break;
        }
    }
}

pub(super) fn check_gl_error(gl: &glow::Context, operation: &'static str) -> Result<()> {
    // SAFETY: this reads the current GL error flag after a GL operation.
    let code = unsafe { gl.get_error() };
    if code == glow::NO_ERROR {
        Ok(())
    } else {
        Err(LinuxGpuInteropError::GlOperation { operation, code })
    }
}
