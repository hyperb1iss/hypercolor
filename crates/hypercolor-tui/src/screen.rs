//! Screen identifiers for TUI navigation.

use std::fmt;

/// Identifies each top-level view in the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ScreenId {
    #[default]
    Dashboard,
    EffectBrowser,
    DeviceManager,
    Profiles,
    Settings,
    Debug,
}

impl ScreenId {
    /// The keybinding that activates this screen.
    #[must_use]
    pub const fn key_hint(self) -> char {
        match self {
            Self::Dashboard => 'D',
            Self::EffectBrowser => 'E',
            Self::DeviceManager => 'V',
            Self::Profiles => 'P',
            Self::Settings => 'S',
            Self::Debug => 'B',
        }
    }

    /// Short label for compact nav.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Dashboard => "Dash",
            Self::EffectBrowser => "Effx",
            Self::DeviceManager => "Devs",
            Self::Profiles => "Prof",
            Self::Settings => "Sttg",
            Self::Debug => "Dbug",
        }
    }

    /// Full display name for the title bar.
    #[must_use]
    pub const fn full_name(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::EffectBrowser => "Effects",
            Self::DeviceManager => "Devices",
            Self::Profiles => "Profiles",
            Self::Settings => "Settings",
            Self::Debug => "Debug",
        }
    }

    /// All screens in nav order.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::Dashboard,
            Self::EffectBrowser,
            Self::DeviceManager,
            Self::Profiles,
            Self::Settings,
            Self::Debug,
        ]
    }

    /// Map a key character to a screen, if any.
    #[must_use]
    pub const fn from_key(c: char) -> Option<Self> {
        match c {
            'D' | 'd' => Some(Self::Dashboard),
            'E' | 'e' => Some(Self::EffectBrowser),
            'V' | 'v' => Some(Self::DeviceManager),
            'P' | 'p' => Some(Self::Profiles),
            'S' | 's' => Some(Self::Settings),
            'B' | 'b' => Some(Self::Debug),
            _ => None,
        }
    }
}

impl fmt::Display for ScreenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}
