use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;

static PROCESS_GL_LOADER: OnceLock<Option<ProcessGlLoader>> = OnceLock::new();

#[derive(Clone, Copy)]
struct ProcessGlLoader {
    lib_gl: Option<usize>,
    lib_egl: Option<usize>,
    glx_get_proc_address: Option<GlxGetProcAddress>,
    egl_get_proc_address: Option<EglGetProcAddress>,
}

// glXGetProcAddress takes `const GLubyte *` (unsigned char), eglGetProcAddress
// takes `const char *` — c_char, whose signedness differs per arch (i8 on
// x86_64, u8 on aarch64), so it must never be hardcoded.
type GlxGetProcAddress = unsafe extern "C" fn(*const u8) -> *const c_void;
type EglGetProcAddress = unsafe extern "C" fn(*const c_char) -> *const c_void;

impl ProcessGlLoader {
    fn load() -> Option<Self> {
        let lib_gl = open_library(c"libGL.so.1").or_else(|| open_library(c"libGL.so"));
        let lib_egl = open_library(c"libEGL.so.1").or_else(|| open_library(c"libEGL.so"));
        let glx_get_proc_address = lib_gl.and_then(|handle| {
            lookup_raw_symbol(handle, c"glXGetProcAddressARB")
                .or_else(|| lookup_raw_symbol(handle, c"glXGetProcAddress"))
                .map(|ptr| {
                    // SAFETY: symbol names are the GLX resolver entry points
                    // with the standard C ABI.
                    unsafe { std::mem::transmute::<*const c_void, GlxGetProcAddress>(ptr) }
                })
        });
        let egl_get_proc_address = lib_egl.and_then(|handle| {
            lookup_raw_symbol(handle, c"eglGetProcAddress").map(|ptr| {
                // SAFETY: symbol name is the EGL resolver entry point with the
                // standard C ABI.
                unsafe { std::mem::transmute::<*const c_void, EglGetProcAddress>(ptr) }
            })
        });

        (lib_gl.is_some()
            || lib_egl.is_some()
            || glx_get_proc_address.is_some()
            || egl_get_proc_address.is_some())
        .then_some(Self {
            lib_gl,
            lib_egl,
            glx_get_proc_address,
            egl_get_proc_address,
        })
    }

    fn lookup(&self, symbol: &CStr) -> *const c_void {
        self.lib_gl
            .and_then(|handle| lookup_raw_symbol(handle, symbol))
            .or_else(|| {
                self.lib_egl
                    .and_then(|handle| lookup_raw_symbol(handle, symbol))
            })
            .or_else(|| {
                self.glx_get_proc_address.and_then(|get_proc_address| {
                    // SAFETY: GLX resolver accepts a NUL-terminated GL symbol
                    // name and returns null when unavailable.
                    let ptr = unsafe { get_proc_address(symbol.as_ptr().cast::<u8>()) };
                    (!ptr.is_null()).then_some(ptr)
                })
            })
            .or_else(|| {
                self.egl_get_proc_address.and_then(|get_proc_address| {
                    // SAFETY: EGL resolver accepts a NUL-terminated GL symbol
                    // name and returns null when unavailable.
                    let ptr = unsafe { get_proc_address(symbol.as_ptr()) };
                    (!ptr.is_null()).then_some(ptr)
                })
            })
            .unwrap_or(std::ptr::null())
    }
}

pub(super) fn process_gl_loader_available() -> bool {
    PROCESS_GL_LOADER
        .get_or_init(ProcessGlLoader::load)
        .is_some()
}

pub(super) fn lookup_process_gl_symbol(symbol: &CStr) -> *const c_void {
    PROCESS_GL_LOADER
        .get_or_init(ProcessGlLoader::load)
        .as_ref()
        .map_or(std::ptr::null(), |loader| loader.lookup(symbol))
}

fn open_library(name: &CStr) -> Option<usize> {
    // SAFETY: dlopen receives a static NUL-terminated library name. Handles are
    // intentionally retained for process lifetime so resolved function pointers
    // remain valid while Servo's GL context is alive.
    let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
    (!handle.is_null()).then_some(handle as usize)
}

fn lookup_raw_symbol(handle: usize, symbol: &CStr) -> Option<*const c_void> {
    // SAFETY: handle came from dlopen and symbol is NUL-terminated.
    let ptr = unsafe { libc::dlsym(handle as *mut c_void, symbol.as_ptr()) };
    (!ptr.is_null()).then_some(ptr.cast_const())
}
