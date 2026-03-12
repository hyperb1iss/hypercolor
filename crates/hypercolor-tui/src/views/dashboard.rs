//! Dashboard view — single-glance overview of the lighting system.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use tokio::sync::mpsc::UnboundedSender;

use crate::action::Action;
use crate::component::Component;
use crate::state::{DaemonState, DeviceSummary, EffectSummary};
use crate::widgets::ParamSlider;

// ── SilkCircuit Neon palette ───────────────────────────────────────────

const NEON_CYAN: Color = Color::Rgb(128, 255, 234);
const ELECTRIC_PURPLE: Color = Color::Rgb(225, 53, 255);
const CORAL: Color = Color::Rgb(255, 106, 193);
const ELECTRIC_YELLOW: Color = Color::Rgb(241, 250, 140);
const SUCCESS_GREEN: Color = Color::Rgb(80, 250, 123);
const ERROR_RED: Color = Color::Rgb(255, 99, 99);
const BASE_WHITE: Color = Color::Rgb(248, 248, 242);
const DIM_GRAY: Color = Color::Rgb(98, 114, 164);
const BORDER_DIM: Color = Color::Rgb(90, 21, 102);

// ── Dashboard View ─────────────────────────────────────────────────────

/// The landing screen: current effect, system health, devices, quick actions.
pub struct DashboardView {
    focused: bool,
    action_tx: Option<UnboundedSender<Action>>,

    // Data
    daemon_state: Option<DaemonState>,
    devices: Vec<DeviceSummary>,
    effects: Vec<EffectSummary>,
    favorites: Vec<String>,
    selected_device: usize,
}

impl Default for DashboardView {
    fn default() -> Self {
        Self::new()
    }
}

impl DashboardView {
    #[must_use]
    pub fn new() -> Self {
        Self {
            focused: false,
            action_tx: None,
            daemon_state: None,
            devices: Vec::new(),
            effects: Vec::new(),
            favorites: Vec::new(),
            selected_device: 0,
        }
    }

    /// Resolve a favorite effect ID to its display name.
    fn favorite_name<'a>(&'a self, id: &'a str) -> &'a str {
        self.effects
            .iter()
            .find(|e| e.id == id)
            .map_or(id, |e| &e.name)
    }

    /// Get the currently active effect, if any.
    fn active_effect(&self) -> Option<&EffectSummary> {
        let id = self.daemon_state.as_ref()?.effect_id.as_deref()?;
        self.effects.iter().find(|e| e.id == id)
    }

    /// Normalize a control value to `[0, 1]` for the slider widget.
    fn normalized_control_value(ctrl: &crate::state::ControlDefinition) -> f32 {
        let raw = ctrl.default_value.as_f32().unwrap_or(0.0);
        let min = ctrl.min.unwrap_or(0.0);
        let max = ctrl.max.unwrap_or(1.0);
        if (max - min).abs() < f32::EPSILON {
            return 0.0;
        }
        ((raw - min) / (max - min)).clamp(0.0, 1.0)
    }

    // ── Panel renderers ─────────────────────────────────────────────

    /// Render the "Current Effect" panel.
    fn render_effect_panel(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(Span::styled(
                " Current Effect ",
                Style::default()
                    .fg(ELECTRIC_PURPLE)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BORDER_DIM));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width < 4 || inner.height < 2 {
            return;
        }

        let Some(effect) = self.active_effect() else {
            let msg = Line::from(Span::styled(
                "No effect active",
                Style::default().fg(DIM_GRAY),
            ));
            frame.render_widget(
                Paragraph::new(msg),
                Rect::new(inner.x + 1, inner.y + 1, inner.width.saturating_sub(2), 1),
            );
            return;
        };

        // Effect name
        let name_line = Line::from(vec![
            Span::styled("\u{2726} ", Style::default().fg(NEON_CYAN)),
            Span::styled(
                &effect.name,
                Style::default().fg(BASE_WHITE).add_modifier(Modifier::BOLD),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(name_line),
            Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), 1),
        );

        // Render up to 4 key parameter sliders
        let max_params = 4.min(effect.controls.len());
        for (i, ctrl) in effect.controls.iter().take(max_params).enumerate() {
            let Some(y_offset) = u16::try_from(i).ok().and_then(|v| v.checked_add(2)) else {
                break;
            };
            let y = inner.y + y_offset;
            if y >= inner.y + inner.height {
                break;
            }
            let value = Self::normalized_control_value(ctrl);
            let slider = ParamSlider::new(&ctrl.name, value).accent_color(NEON_CYAN);
            let slider_area = Rect::new(inner.x + 1, y, inner.width.saturating_sub(2), 1);
            frame.render_widget(slider, slider_area);
        }
    }

    /// Render the "System Health" panel.
    fn render_health_panel(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(Span::styled(
                " System Health ",
                Style::default()
                    .fg(ELECTRIC_PURPLE)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BORDER_DIM));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width < 4 || inner.height < 2 {
            return;
        }

        let lines = self.build_health_lines();
        frame.render_widget(
            Paragraph::new(lines),
            Rect::new(
                inner.x + 1,
                inner.y,
                inner.width.saturating_sub(2),
                inner.height,
            ),
        );
    }

    /// Build the health status lines.
    fn build_health_lines(&self) -> Vec<Line<'_>> {
        let Some(ref ds) = self.daemon_state else {
            return vec![Line::from(Span::styled(
                "Daemon    \u{25CF} disconnected",
                Style::default().fg(ERROR_RED),
            ))];
        };

        let status_style = if ds.running {
            Style::default().fg(SUCCESS_GREEN)
        } else {
            Style::default().fg(ERROR_RED)
        };
        let status_text = if ds.running { "running" } else { "stopped" };

        vec![
            Line::from(vec![
                Span::styled("Daemon    ", Style::default().fg(DIM_GRAY)),
                Span::styled("\u{25CF} ", status_style),
                Span::styled(status_text, status_style),
            ]),
            Line::from(vec![
                Span::styled("Render    ", Style::default().fg(DIM_GRAY)),
                Span::styled(
                    format!("{:.1} fps", ds.fps_actual),
                    Style::default().fg(CORAL),
                ),
                Span::styled(
                    format!("  / {:.0} target", ds.fps_target),
                    Style::default().fg(DIM_GRAY),
                ),
            ]),
            Line::from(vec![
                Span::styled("Devices   ", Style::default().fg(DIM_GRAY)),
                Span::styled(format!("{}", ds.device_count), Style::default().fg(CORAL)),
                Span::styled(
                    format!("  ({} LEDs)", ds.total_leds),
                    Style::default().fg(DIM_GRAY),
                ),
            ]),
            Line::from(vec![
                Span::styled("Bright    ", Style::default().fg(DIM_GRAY)),
                Span::styled(format!("{}%", ds.brightness), Style::default().fg(CORAL)),
            ]),
        ]
    }

    /// Render the "Connected Devices" table.
    fn render_devices_table(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(Span::styled(
                " Connected Devices ",
                Style::default()
                    .fg(ELECTRIC_PURPLE)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BORDER_DIM));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width < 10 || inner.height < 3 {
            return;
        }

        let header_style = Style::default().fg(DIM_GRAY).add_modifier(Modifier::BOLD);
        let header = Row::new(vec![
            Cell::from("Device").style(header_style),
            Cell::from("Type").style(header_style),
            Cell::from("LEDs").style(header_style),
            Cell::from("Status").style(header_style),
            Cell::from("FPS").style(header_style),
        ]);

        let rows: Vec<Row<'_>> = self
            .devices
            .iter()
            .enumerate()
            .map(|(i, dev)| self.build_device_row(i, dev))
            .collect();

        let total_leds: u32 = self.devices.iter().map(|d| d.led_count).sum();
        let footer_text = format!(
            "Total: {total_leds} LEDs across {} devices",
            self.devices.len()
        );

        let widths = [
            Constraint::Percentage(30),
            Constraint::Percentage(20),
            Constraint::Percentage(15),
            Constraint::Percentage(20),
            Constraint::Percentage(15),
        ];

        let table = Table::new(rows, widths)
            .header(header)
            .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

        frame.render_widget(
            table,
            Rect::new(
                inner.x,
                inner.y,
                inner.width,
                inner.height.saturating_sub(1),
            ),
        );

        // Footer line
        let footer_y = inner.y + inner.height.saturating_sub(1);
        if footer_y < inner.y + inner.height {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    footer_text,
                    Style::default().fg(DIM_GRAY),
                ))),
                Rect::new(inner.x + 1, footer_y, inner.width.saturating_sub(2), 1),
            );
        }
    }

    /// Build a single device row for the table.
    fn build_device_row<'a>(&self, idx: usize, dev: &'a DeviceSummary) -> Row<'a> {
        let is_selected = idx == self.selected_device;
        let name_style = if is_selected {
            Style::default().fg(NEON_CYAN).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(BASE_WHITE)
        };

        let status_style = if dev.state == "ok" || dev.state == "connected" {
            Style::default().fg(SUCCESS_GREEN)
        } else {
            Style::default().fg(ERROR_RED)
        };

        let fps_text = dev
            .fps
            .map_or_else(|| "-".to_string(), |f| format!("{f:.0}"));

        Row::new(vec![
            Cell::from(dev.name.clone()).style(name_style),
            Cell::from(dev.family.clone()).style(Style::default().fg(DIM_GRAY)),
            Cell::from(dev.led_count.to_string()).style(Style::default().fg(CORAL)),
            Cell::from(format!("\u{25CF} {}", dev.state)).style(status_style),
            Cell::from(fps_text).style(Style::default().fg(CORAL)),
        ])
    }

    /// Render the "Quick Actions" row.
    fn render_quick_actions(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(Span::styled(
                " Quick Actions ",
                Style::default()
                    .fg(ELECTRIC_PURPLE)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BORDER_DIM));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width < 4 || inner.height < 1 {
            return;
        }

        let mut spans: Vec<Span<'_>> = Vec::new();
        for (i, fav_id) in self.favorites.iter().take(5).enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                format!("[{}]", i + 1),
                Style::default().fg(ELECTRIC_YELLOW),
            ));
            spans.push(Span::styled(
                format!(" {}", self.favorite_name(fav_id)),
                Style::default().fg(BASE_WHITE),
            ));
        }

        if spans.is_empty() {
            spans.push(Span::styled(
                "No favorites \u{2014} press [f] in the effect browser to add some",
                Style::default().fg(DIM_GRAY),
            ));
        }

        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), 1),
        );
    }
}

impl Component for DashboardView {
    fn init(&mut self, action_tx: UnboundedSender<Action>) -> Result<()> {
        self.action_tx = Some(action_tx);
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        match key.code {
            // Number keys 1-5 for quick favorite effects
            KeyCode::Char(c @ '1'..='5') => {
                // Safe: c is in '1'..='5', so the subtraction fits in usize.
                #[allow(clippy::as_conversions)]
                let idx = (u32::from(c) - u32::from('1')) as usize;
                if let Some(fav_id) = self.favorites.get(idx) {
                    return Ok(Some(Action::ApplyEffect(fav_id.clone())));
                }
                Ok(None)
            }
            // Device navigation
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.devices.is_empty() {
                    self.selected_device =
                        (self.selected_device + 1).min(self.devices.len().saturating_sub(1));
                }
                Ok(None)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected_device = self.selected_device.saturating_sub(1);
                Ok(None)
            }
            KeyCode::Enter => Ok(Some(Action::SwitchScreen(
                crate::screen::ScreenId::DeviceManager,
            ))),
            _ => Ok(None),
        }
    }

    fn update(&mut self, action: &Action) -> Result<Option<Action>> {
        match action {
            Action::DaemonStateUpdated(state) | Action::DaemonConnected(state) => {
                self.daemon_state = Some(*state.clone());
            }
            Action::DevicesUpdated(devices) => {
                self.devices.clone_from(devices);
                if self.devices.is_empty() {
                    self.selected_device = 0;
                } else {
                    self.selected_device = self
                        .selected_device
                        .min(self.devices.len().saturating_sub(1));
                }
            }
            Action::EffectsUpdated(effects) => {
                self.effects.clone_from(effects);
            }
            Action::FavoritesUpdated(favs) => {
                self.favorites.clone_from(favs);
            }
            Action::DaemonDisconnected(_) => {
                self.daemon_state = None;
            }
            _ => {}
        }
        Ok(None)
    }

    fn render(&self, frame: &mut Frame, area: Rect) {
        if area.width < 4 || area.height < 4 {
            return;
        }

        // Layout: 2-column top (effect + health), devices table, quick actions
        let vertical = Layout::vertical([
            Constraint::Min(8),
            Constraint::Fill(1),
            Constraint::Length(3),
        ])
        .split(area);

        let top_cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(vertical[0]);

        self.render_effect_panel(frame, top_cols[0]);
        self.render_health_panel(frame, top_cols[1]);
        self.render_devices_table(frame, vertical[1]);
        self.render_quick_actions(frame, vertical[2]);
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn id(&self) -> &'static str {
        "dashboard"
    }
}
