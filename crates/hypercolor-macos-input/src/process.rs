#[cfg(target_os = "macos")]
use mach2::message::audit_token_t;
#[cfg(target_os = "macos")]
use mach2::task::task_info;
#[cfg(target_os = "macos")]
use mach2::task_info::{TASK_AUDIT_TOKEN, TASK_AUDIT_TOKEN_COUNT, task_info_t};
#[cfg(target_os = "macos")]
use mach2::traps::mach_task_self;

use crate::{MacosInputError, MacosInputResult};

/// Return the current process audit token as eight fixed-width hexadecimal words.
pub fn current_process_audit_token_identity() -> MacosInputResult<String> {
    #[cfg(not(target_os = "macos"))]
    {
        Err(MacosInputError::UnsupportedPlatform)
    }

    #[cfg(target_os = "macos")]
    {
        let mut token = audit_token_t::default();
        let mut count = TASK_AUDIT_TOKEN_COUNT;
        // SAFETY: task_info writes exactly TASK_AUDIT_TOKEN_COUNT natural_t
        // values into a correctly aligned audit_token_t owned by this call.
        let result = unsafe {
            task_info(
                mach_task_self(),
                TASK_AUDIT_TOKEN,
                std::ptr::from_mut(&mut token).cast::<mach2::vm_types::integer_t>() as task_info_t,
                &mut count,
            )
        };
        if result != mach2::kern_return::KERN_SUCCESS {
            return Err(MacosInputError::AuditToken(result));
        }
        if count != TASK_AUDIT_TOKEN_COUNT {
            return Err(MacosInputError::AuditToken(result));
        }
        Ok(token.val.map(|word| format!("{word:08x}")).join(":"))
    }
}
