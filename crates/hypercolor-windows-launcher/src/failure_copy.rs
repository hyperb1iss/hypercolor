//! What a person reads when the launcher cannot start Hypercolor.

use hypercolor_install::windows::ReleaseSelectionError;

/// The launcher's dialog title.
pub const FAILURE_TITLE: &str = "Hypercolor";

/// The dialog's message for a launch that never got as far as starting a
/// release. A first install still in flight resolves itself; anything else
/// means the installation needs repair, whatever record gave out.
#[must_use]
pub fn failure_message(selection: Option<&ReleaseSelectionError>) -> &'static str {
    match selection {
        Some(ReleaseSelectionError::NoPrior) => {
            "Hypercolor is still being set up. Try again in a moment."
        }
        _ => {
            "Hypercolor could not start because this installation is damaged. \
              Reinstall Hypercolor to repair it."
        }
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_install::windows::ReleaseSelectionError;

    use super::failure_message;

    #[test]
    fn a_first_install_in_flight_only_asks_for_patience() {
        assert!(failure_message(Some(&ReleaseSelectionError::NoPrior)).contains("being set up"));
    }

    #[test]
    fn anything_else_points_at_a_reinstall_and_says_installation() {
        for message in [
            failure_message(None),
            failure_message(Some(&ReleaseSelectionError::NoActive)),
            failure_message(Some(&ReleaseSelectionError::Unsettled)),
        ] {
            assert!(message.contains("installation"), "{message}");
            assert!(message.contains("Reinstall"), "{message}");
        }
    }
}
