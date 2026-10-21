//! Microphone record permission, read without ever requesting it.
//!
//! macOS gates every audio input device, virtual loopback drivers included,
//! behind the Microphone privacy permission. Opening an input before the user
//! has answered shows the system prompt, so callers that open inputs on their
//! own initiative read the current answer here first.

use hypercolor_types::audio::AudioInputPermission;
use objc2::rc::autoreleasepool;
use objc2_avf_audio::{AVAudioApplication, AVAudioApplicationRecordPermission};

/// Report this process's current microphone record permission.
///
/// This only reads the stored answer through `AVAudioApplication`; it never
/// requests access, so calling it cannot show the macOS microphone prompt.
#[must_use]
pub fn microphone_record_permission() -> AudioInputPermission {
    autoreleasepool(|_| {
        // SAFETY: `sharedInstance` takes no arguments and returns the
        // process-wide AVAudioApplication singleton as a retained object.
        // The class exists from macOS 14, below the 15.2 deployment floor.
        let application = unsafe { AVAudioApplication::sharedInstance() };
        // SAFETY: `recordPermission` is a read-only property getter on the
        // live singleton retained above. It queries the stored privacy
        // answer and has no side effects.
        let permission = unsafe { application.recordPermission() };
        record_permission(permission)
    })
}

fn record_permission(permission: AVAudioApplicationRecordPermission) -> AudioInputPermission {
    match permission {
        AVAudioApplicationRecordPermission::Granted => AudioInputPermission::Granted,
        AVAudioApplicationRecordPermission::Denied => AudioInputPermission::Denied,
        // Undetermined, plus any value a later SDK introduces: access is not
        // confirmed, so nothing may open an input on the strength of it.
        _ => AudioInputPermission::Undetermined,
    }
}
