use std::time::Instant;

use glow::HasContext;

use super::gl_external_memory::check_gl_error;
use super::{LinuxGpuInteropError, Result, elapsed_micros};

const GL_TIMEOUT_IGNORED_NS: i32 = -1;

pub(super) enum GlFenceStatus {
    Complete,
    Pending,
}

pub(super) fn create_gl_fence(gl: &glow::Context) -> Result<glow::NativeFence> {
    // SAFETY: the caller holds the current GL context and the fence is inserted
    // after the framebuffer blit commands issued on that same context.
    let fence = unsafe {
        gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0)
            .map_err(|message| LinuxGpuInteropError::GlCreateResource {
                resource: "sync object",
                message,
            })?
    };
    if fence.0.is_null() {
        return Err(LinuxGpuInteropError::GlCreateResource {
            resource: "sync object",
            message: "glFenceSync returned null".to_owned(),
        });
    }
    check_gl_error(gl, "glFenceSync")?;
    Ok(fence)
}

pub(super) fn wait_for_gl_blit_completion(gl: &glow::Context) -> Result<u64> {
    let fence = create_gl_fence(gl)?;
    let result = wait_for_gl_fence_completion(gl, fence);
    delete_gl_fence(gl, fence);
    result
}

pub(super) fn wait_for_gl_fence_completion(
    gl: &glow::Context,
    fence: glow::NativeFence,
) -> Result<u64> {
    let sync_start = Instant::now();
    // SAFETY: `fence` was created in this context above and remains live until
    // the caller deletes it after the wait returns.
    let status =
        unsafe { gl.client_wait_sync(fence, glow::SYNC_FLUSH_COMMANDS_BIT, GL_TIMEOUT_IGNORED_NS) };
    let sync_us = elapsed_micros(sync_start);
    check_gl_error(gl, "glClientWaitSync")?;

    match status {
        glow::ALREADY_SIGNALED | glow::CONDITION_SATISFIED => Ok(sync_us),
        code => Err(LinuxGpuInteropError::GlOperation {
            operation: "glClientWaitSync",
            code,
        }),
    }
}

pub(super) fn poll_gl_fence(gl: &glow::Context, fence: glow::NativeFence) -> Result<GlFenceStatus> {
    // SAFETY: `fence` was created in this context and remains live while owned
    // by the pending import slot.
    let status = unsafe { gl.client_wait_sync(fence, 0, 0) };
    check_gl_error(gl, "glClientWaitSync")?;

    match status {
        glow::ALREADY_SIGNALED | glow::CONDITION_SATISFIED => Ok(GlFenceStatus::Complete),
        glow::TIMEOUT_EXPIRED => Ok(GlFenceStatus::Pending),
        code => Err(LinuxGpuInteropError::GlOperation {
            operation: "glClientWaitSync",
            code,
        }),
    }
}

pub(super) fn delete_gl_fence(gl: &glow::Context, fence: glow::NativeFence) {
    // SAFETY: the fence belongs to this current GL context and is no longer
    // needed by the caller.
    unsafe {
        gl.delete_sync(fence);
    }
}
