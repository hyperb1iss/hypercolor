use super::model::WindowsAppStart;
use crate::InstallPlatformError;

/// The recorded user's session, as the update service reaches it.
///
/// The app runs in that session as that user, so a LocalSystem service can
/// reach it only through two seams it owns: the update pipe the app holds
/// open, and the token of the user's session (`WTSEnumerateSessionsW`,
/// `WTSQueryUserToken`, `CreateProcessAsUserW`).
pub trait WindowsSessionHost {
    /// Ask the running app to quit over the update pipe. The app closes its
    /// window, ends its daemon's job and exits. Returns whether the app took
    /// the request; one that cannot be reached is stopped by force at once.
    fn ask_app_to_quit(&mut self) -> bool;

    /// Start `app` with `arguments` in the recorded user's active session,
    /// as that user, the way a sign-in starts it.
    ///
    /// # Errors
    /// Returns an error when a session exists but the app cannot be started
    /// in it. A user without a session is [`WindowsAppStart::NoSession`].
    fn start_app(
        &mut self,
        app: &str,
        arguments: &[&str],
    ) -> Result<WindowsAppStart, InstallPlatformError>;
}
