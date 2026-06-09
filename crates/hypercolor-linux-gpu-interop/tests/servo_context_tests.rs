#![cfg(all(target_os = "linux", feature = "servo-context"))]

use glow::HasContext;
use hypercolor_linux_gpu_interop::LinuxServoRenderDevice;
use servo::RenderingContext;
use std::rc::Rc;

const RUN_FIXTURE_ENV: &str = "HYPERCOLOR_RUN_GPU_INTEROP_FIXTURE";

#[test]
fn linux_servo_context_exposes_current_framebuffer() {
    if std::env::var_os(RUN_FIXTURE_ENV).is_none() {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the Servo context fixture");
        return;
    }

    let parent = Rc::new(
        LinuxServoRenderDevice::new_software(4, 4)
            .expect("Servo context fixture should create software context"),
    );
    let context = parent
        .create_rendering_context(4, 4)
        .expect("Servo context fixture should create render target");
    context
        .make_current()
        .expect("Servo context fixture should become current");
    context.prepare_for_rendering();

    let framebuffer = context
        .framebuffer()
        .expect("Servo context fixture should expose an FBO");
    let snapshot = context.target_snapshot();
    assert_ne!(snapshot.framebuffer, 0);

    assert_framebuffer_complete(&context, framebuffer);
}

#[test]
fn linux_servo_contexts_share_parent_with_distinct_framebuffers() {
    if std::env::var_os(RUN_FIXTURE_ENV).is_none() {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the Servo context fixture");
        return;
    }

    let parent = Rc::new(
        LinuxServoRenderDevice::new_software(8, 8)
            .expect("Servo context fixture should create software context"),
    );
    let first = parent
        .create_rendering_context(8, 8)
        .expect("first render target should be created");
    let second = parent
        .create_rendering_context(8, 8)
        .expect("second render target should be created");

    let first_framebuffer = first
        .framebuffer()
        .expect("first context should expose an FBO");
    let second_framebuffer = second
        .framebuffer()
        .expect("second context should expose an FBO");
    assert_ne!(
        first.target_snapshot().framebuffer,
        second.target_snapshot().framebuffer
    );

    assert_framebuffer_complete(&first, first_framebuffer);
    assert_framebuffer_complete(&second, second_framebuffer);
}

fn assert_framebuffer_complete(
    context: &impl RenderingContext,
    framebuffer: glow::NativeFramebuffer,
) {
    context
        .make_current()
        .expect("Servo context fixture should become current");
    context.prepare_for_rendering();

    let gl = context.glow_gl_api();
    // SAFETY: the framebuffer comes from the current Servo GL context.
    unsafe {
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(framebuffer));
        assert_eq!(
            gl.check_framebuffer_status(glow::READ_FRAMEBUFFER),
            glow::FRAMEBUFFER_COMPLETE
        );
    }
}
