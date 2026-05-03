//! Command-line flags for app lifecycle control.

/// Parsed app lifecycle arguments.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AppArgs {
    /// Start the main window hidden.
    pub start_minimized: bool,
    /// Show the already-running app instance.
    pub show: bool,
    /// Ask the already-running app instance to quit.
    pub quit: bool,
}

impl AppArgs {
    /// Parse process arguments.
    ///
    /// Unknown arguments are ignored so forwarded OS/runtime flags do not block
    /// app startup.
    #[must_use]
    pub fn parse<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut parsed = Self::default();

        for arg in args {
            match arg.as_ref() {
                "--minimized" | "--hidden" => parsed.start_minimized = true,
                "--show" => parsed.show = true,
                "--quit" => parsed.quit = true,
                _ => {}
            }
        }

        parsed
    }

    /// Parse the current process arguments.
    #[must_use]
    pub fn parse_env() -> Self {
        Self::parse(std::env::args().skip(1))
    }
}
