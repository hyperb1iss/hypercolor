// Intentionally console-subsystem: `just daemon`, `hypercolor-daemon --help`,
// and other direct CLI invocations need stdout/stderr attached to the user's
// terminal. The supervisor inside hypercolor-app hides its child via
// CREATE_NO_WINDOW (see `configure_platform_command` in supervisor::mod), so
// the GUI shell path also stays clean without forcing GUI subsystem here.

use anyhow::Result;

fn main() -> Result<()> {
    hypercolor_daemon::process::run(&[])
}
