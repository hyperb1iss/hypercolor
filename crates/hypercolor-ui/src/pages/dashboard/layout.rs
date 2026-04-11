//! Dashboard layout state — per-panel visibility, width, and order.
//!
//! Every panel in the stats section below the hero row is represented by
//! a [`PanelConfig`] in [`DashboardLayout`]. The layout is loaded from
//! `localStorage` on mount, rendered as a 6-column CSS grid with
//! per-panel column spans, and written back whenever the user drags a
//! panel to a new position, cycles its width, or hides it from view.
//!
//! The layout is forward-compatible: when a newer build adds a panel,
//! [`DashboardLayout::load`] appends any missing IDs to the saved
//! layout so the user sees the new panel without having to reset.

use serde::{Deserialize, Serialize};

use crate::storage;

const STORAGE_KEY: &str = "hc-dashboard-layout";

/// One of the nine dashboard stats panels. Closed set — adding a panel
/// means adding a variant here and updating [`PanelId::ALL`] plus the
/// render match in `mod.rs`. Serialised in snake_case to keep the
/// localStorage blob human-readable.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PanelId {
    HeroGauges,
    Pipeline,
    FrameTimeline,
    Distribution,
    Pacing,
    ReuseRates,
    MemoryAndDevices,
    Throughput,
    LatestFrame,
}

impl PanelId {
    /// All panel IDs in their default display order.
    pub const ALL: [PanelId; 9] = [
        PanelId::HeroGauges,
        PanelId::Pipeline,
        PanelId::FrameTimeline,
        PanelId::Distribution,
        PanelId::Pacing,
        PanelId::ReuseRates,
        PanelId::MemoryAndDevices,
        PanelId::Throughput,
        PanelId::LatestFrame,
    ];

    /// Short, human-readable label for menus and drag tooltips.
    pub fn label(self) -> &'static str {
        match self {
            PanelId::HeroGauges => "Render Engine",
            PanelId::Pipeline => "Pipeline Breakdown",
            PanelId::FrameTimeline => "Frame Timeline",
            PanelId::Distribution => "Frame Distribution",
            PanelId::Pacing => "Pacing",
            PanelId::ReuseRates => "Reuse Rates",
            PanelId::MemoryAndDevices => "Memory & Devices",
            PanelId::Throughput => "WS Throughput",
            PanelId::LatestFrame => "Latest Frame",
        }
    }
}

/// Column width a panel occupies on the dashboard's 6-column grid.
/// Full spans all 6 columns (one panel per row), Half spans 3 (two per
/// row), Third spans 2 (three per row). At narrower viewports the grid
/// gracefully falls back to full-width on everything.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PanelWidth {
    Full,
    Half,
    Third,
}

impl PanelWidth {
    /// Tailwind class for the panel's col-span on the `xl` breakpoint,
    /// where we commit to the multi-column layout. Below `xl` every
    /// panel collapses to full width (handled separately in `mod.rs`).
    pub fn xl_col_span_class(self) -> &'static str {
        match self {
            PanelWidth::Full => "xl:col-span-6",
            PanelWidth::Half => "xl:col-span-3",
            PanelWidth::Third => "xl:col-span-2",
        }
    }

    /// Cycle through Full → Half → Third → Full for the width toggle
    /// button in each panel's floating control bar.
    pub fn next(self) -> PanelWidth {
        match self {
            PanelWidth::Full => PanelWidth::Half,
            PanelWidth::Half => PanelWidth::Third,
            PanelWidth::Third => PanelWidth::Full,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PanelWidth::Full => "full",
            PanelWidth::Half => "half",
            PanelWidth::Third => "third",
        }
    }
}

fn default_visible() -> bool {
    true
}

fn default_width() -> PanelWidth {
    PanelWidth::Full
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PanelConfig {
    pub id: PanelId,
    #[serde(default = "default_visible")]
    pub visible: bool,
    #[serde(default = "default_width")]
    pub width: PanelWidth,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DashboardLayout {
    pub panels: Vec<PanelConfig>,
}

impl DashboardLayout {
    /// Ships-by-default arrangement — matches the hardcoded stats
    /// section we had before the layout system was introduced. The two
    /// half-width pairs (distribution/pacing and reuse/memory) preserve
    /// the previous `xl:grid-cols-2` rows as their starting layout.
    pub fn default_layout() -> Self {
        Self {
            panels: vec![
                PanelConfig {
                    id: PanelId::HeroGauges,
                    visible: true,
                    width: PanelWidth::Full,
                },
                PanelConfig {
                    id: PanelId::Pipeline,
                    visible: true,
                    width: PanelWidth::Full,
                },
                PanelConfig {
                    id: PanelId::FrameTimeline,
                    visible: true,
                    width: PanelWidth::Full,
                },
                PanelConfig {
                    id: PanelId::Distribution,
                    visible: true,
                    width: PanelWidth::Half,
                },
                PanelConfig {
                    id: PanelId::Pacing,
                    visible: true,
                    width: PanelWidth::Half,
                },
                PanelConfig {
                    id: PanelId::ReuseRates,
                    visible: true,
                    width: PanelWidth::Half,
                },
                PanelConfig {
                    id: PanelId::MemoryAndDevices,
                    visible: true,
                    width: PanelWidth::Half,
                },
                PanelConfig {
                    id: PanelId::Throughput,
                    visible: true,
                    width: PanelWidth::Full,
                },
                PanelConfig {
                    id: PanelId::LatestFrame,
                    visible: true,
                    width: PanelWidth::Full,
                },
            ],
        }
    }

    /// Loads the saved layout from `localStorage`, falling back to the
    /// default on missing or malformed data. Also reconciles the loaded
    /// layout against [`PanelId::ALL`]: any panels that were added in
    /// newer builds get appended (visible, full-width) so upgrades
    /// don't silently hide new panels, and any unknown panels from
    /// older builds are dropped.
    pub fn load() -> Self {
        let parsed = storage::get(STORAGE_KEY)
            .and_then(|raw| serde_json::from_str::<DashboardLayout>(&raw).ok());
        let Some(mut layout) = parsed else {
            return Self::default_layout();
        };

        layout.panels.retain(|p| PanelId::ALL.contains(&p.id));
        for id in PanelId::ALL {
            if !layout.panels.iter().any(|p| p.id == id) {
                layout.panels.push(PanelConfig {
                    id,
                    visible: true,
                    width: PanelWidth::Full,
                });
            }
        }
        layout
    }

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_string(self) {
            storage::set(STORAGE_KEY, &json);
        }
    }

    /// Moves the panel at `from` to land immediately before the panel
    /// at `target`. Handles the shift-after-removal dance so dragging
    /// forward and dragging backward both land where the user expects.
    pub fn move_panel(&mut self, from: usize, target: usize) {
        if from >= self.panels.len() || target >= self.panels.len() || from == target {
            return;
        }
        let panel = self.panels.remove(from);
        // When moving forward, the items after the source have all
        // shifted down by one, so subtract one from the target to
        // keep "drop before target" semantics.
        let insert_at = if from < target {
            target.saturating_sub(1)
        } else {
            target
        };
        self.panels.insert(insert_at.min(self.panels.len()), panel);
    }

    pub fn set_visible(&mut self, id: PanelId, visible: bool) {
        if let Some(p) = self.panels.iter_mut().find(|p| p.id == id) {
            p.visible = visible;
        }
    }

    pub fn cycle_width(&mut self, id: PanelId) {
        if let Some(p) = self.panels.iter_mut().find(|p| p.id == id) {
            p.width = p.width.next();
        }
    }

    /// Any hidden panels? Drives the "show hidden" indicator in the
    /// gear menu.
    pub fn has_hidden(&self) -> bool {
        self.panels.iter().any(|p| !p.visible)
    }
}

impl Default for DashboardLayout {
    fn default() -> Self {
        Self::default_layout()
    }
}
