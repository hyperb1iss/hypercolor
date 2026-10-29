//! Portable presentation preferences for the terminal client.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TuiPreferences {
    pub theme: Option<String>,
    pub show_donate: bool,
}

impl Default for TuiPreferences {
    fn default() -> Self {
        Self {
            theme: None,
            show_donate: true,
        }
    }
}
