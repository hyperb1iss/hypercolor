#![cfg(all(target_os = "linux", feature = "servo-context"))]

use glow::HasContext;
use hypercolor_linux_gpu_interop::LinuxServoRenderingContext;
use servo::RenderingContext;

const RUN_FIXTURE_ENV: &str = "HYPERCOLOR_RUN_GPU_INTEROP_FIXTURE";

#[test]
fn linux_servo_context_exposes_current_framebuffer() {
    if std::env::var_os(RUN_FIXTURE_ENV).is_none() {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the Servo context fixture");
        return;
    }

    let context = LinuxServoRenderingContext::new_software(4, 4)
        .expect("Servo context fixture should create software context");
    context
        .make_current()
        .expect("Servo context fixture should become current");
    context.prepare_for_rendering();

    let framebuffer = context
        .framebuffer()
        .expect("Servo context fixture should expose a surfman FBO");
    let snapshot = context
        .surface_snapshot()
        .expect("Servo context fixture should expose a surface snapshot");
    assert_ne!(snapshot.framebuffer, 0);

    assert_framebuffer_complete(&context, framebuffer);
}

#[test]
fn linux_servo_context_refreshes_after_peer_context_drop() {
    if std::env::var_os(RUN_FIXTURE_ENV).is_none() {
        eprintln!("set {RUN_FIXTURE_ENV}=1 to run the Servo context fixture");
        return;
    }

    let survivor = LinuxServoRenderingContext::new_software(4, 4)
        .expect("Servo context fixture should create survivor context");
    survivor
        .make_current()
        .expect("survivor context should become current");
    survivor.prepare_for_rendering();
    survivor.present();

    let peer = LinuxServoRenderingContext::new_software(4, 4)
        .expect("Servo context fixture should create peer context");
    drop(peer);

    survivor
        .refresh_surface()
        .expect("survivor context should refresh its surfman surface");
    survivor.prepare_for_rendering();

    let framebuffer = survivor
        .framebuffer()
        .expect("refreshed survivor context should expose a surfman FBO");
    assert_framebuffer_complete(&survivor, framebuffer);
}

fn assert_framebuffer_complete(
    context: &LinuxServoRenderingContext,
    framebuffer: glow::NativeFramebuffer,
) {
    context
        .make_current()
        .expect("Servo context fixture should become current");
    context.prepare_for_rendering();

    let gl = context.glow_gl_api();
    // SAFETY: the framebuffer comes from the current Servo surfman context.
    unsafe {
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(framebuffer));
        assert_eq!(
            gl.check_framebuffer_status(glow::READ_FRAMEBUFFER),
            glow::FRAMEBUFFER_COMPLETE
        );
    }
}
