//! Dashboard view — single-glance overview of the lighting system.

use std::cell::Cell as StdCell;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};
use tokio::sync::mpsc::UnboundedSender;

use crate::action::Action;
use crate::component::Component;
use crate::state::{
    CanvasPreviewState, ConnectionStatus, DaemonState, DeviceSummary, EffectSummary,
};
use crate::widgets::{ParamSlider, Split, SplitDirection};

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

/// The landing screen: current effect info, live canvas preview, devices, quick actions.
pub struct DashboardView {
    focused: bool,
    action_tx: Option<UnboundedSender<Action>>,

    // Resizable panel splits
    /// Vertical: top section / devices table (quick actions stays fixed at bottom)
    v_split: Split,
    /// Horizontal: effect panel / preview panel (within top section)
    h_split: Split,

    // Data
    daemon_state: Option<DaemonState>,
    devices: Vec<DeviceSummary>,
    effects: Vec<EffectSummary>,
    favorites: Vec<String>,
    canvas_frame: Option<CanvasPreviewState>,
    selected_device: usize,
    connection_status: ConnectionStatus,
    disconnect_reason: Option<String>,

    /// Last computed inner area of the preview panel, exposed via the
    /// Component trait so App can overlay the live ratatui-image protocol
    /// (Kitty/Sixel/halfblocks) on top.
    preview_inner: StdCell<Option<Rect>>,
    devices_rect: StdCell<Rect>,
    actions_rect: StdCell<Rect>,
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
            v_split: Split::new(SplitDirection::Vertical, 0.4).min_sizes(6, 5),
            h_split: Split::new(SplitDirection::Horizontal, 0.4).min_sizes(15, 15),
            daemon_state: None,
            devices: Vec::new(),
            effects: Vec::new(),
            favorites: Vec::new(),
            canvas_frame: None,
            selected_device: 0,
            connection_status: ConnectionStatus::default(),
            disconnect_reason: None,
            preview_inner: StdCell::new(None),
            devices_rect: StdCell::new(Rect::default()),
            actions_rect: StdCell::new(Rect::default()),
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

    // ── Panel renderers ─────────────────────────────────────────────

    /// Render the "Now Playing" panel — effect info + daemon gauges.
    ///
    /// Layout (top to bottom):
    ///   ▶ Effect Name                               (1 row)
    ///   description text wrapped natively           (0–3 rows)
    ///   author · category                           (0–1 row)
    ///   ─────────────                               (1 row)
    ///   ♫ Audio  ⚙ N ctrl  ★ N pre                  (0–1 row)
    ///   tag tag tag tag                             (0–2 rows, wrapped)
    ///   ─────────────                               (1 row)
    ///   Bright  ████░░░░ 100%                       (1 row)
    ///   FPS     60 / 60                             (1 row)
    ///
    /// Each section is laid out top-down with bounds checks so partial
    /// rendering survives narrow / short panel sizes.
    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    fn render_effect_panel(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(Span::styled(
                " Now Playing ",
                Style::default()
                    .fg(ELECTRIC_PURPLE)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BORDER_DIM));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width < 6 || inner.height < 2 {
            return;
        }

        // 2-cell horizontal padding inside the block for breathing room
        let pad_x = 2_u16;
        let content_w = inner.width.saturating_sub(pad_x * 2);
        let x = inner.x + pad_x;
        let max_y = inner.y + inner.height;

        let Some(effect) = self.active_effect() else {
            self.render_idle_state(
                frame,
                Rect::new(x, inner.y + 1, content_w, max_y.saturating_sub(inner.y + 1)),
            );
            return;
        };

        let mut y = inner.y + 1; // 1-row top padding

        // ── Effect identity ──────────────────────────
        y = Self::render_effect_header(frame, effect, x, y, max_y, content_w);
        y = Self::render_effect_description(frame, effect, x, y, max_y, content_w);
        y = Self::render_author_category(frame, effect, x, y, max_y, content_w);
        y = Self::render_separator(frame, x, y, max_y, content_w);

        // ── Effect features ──────────────────────────
        y = Self::render_feature_badges(frame, effect, x, y, max_y, content_w);
        y = Self::render_tags(frame, effect, x, y, max_y, content_w);
        y = Self::render_separator(frame, x, y, max_y, content_w);

        // ── Daemon gauges ────────────────────────────
        self.render_daemon_gauges(frame, x, y, max_y, content_w);
    }

    /// Effect name with play indicator.
    fn render_effect_header(
        frame: &mut Frame,
        effect: &EffectSummary,
        x: u16,
        y: u16,
        max_y: u16,
        w: u16,
    ) -> u16 {
        if y >= max_y {
            return y;
        }
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("\u{25B6} ", Style::default().fg(SUCCESS_GREEN)),
                Span::styled(
                    truncate_str(&effect.name, w.saturating_sub(2).into()),
                    Style::default().fg(BASE_WHITE).add_modifier(Modifier::BOLD),
                ),
            ])),
            Rect::new(x, y, w, 1),
        );
        y + 1
    }

    /// Description text — natively wrapped via `Wrap { trim: false }`,
    /// capped at 3 rows.
    fn render_effect_description(
        frame: &mut Frame,
        effect: &EffectSummary,
        x: u16,
        y: u16,
        max_y: u16,
        w: u16,
    ) -> u16 {
        if effect.description.is_empty() || y >= max_y {
            return y;
        }
        let max_h = (max_y - y).min(3);
        if max_h == 0 {
            return y;
        }
        frame.render_widget(
            Paragraph::new(Span::styled(
                effect.description.as_str(),
                Style::default().fg(DIM_GRAY),
            ))
            .wrap(Wrap { trim: false }),
            Rect::new(x, y, w, max_h),
        );
        y + max_h
    }

    /// Author · category meta line.
    fn render_author_category(
        frame: &mut Frame,
        effect: &EffectSummary,
        x: u16,
        y: u16,
        max_y: u16,
        w: u16,
    ) -> u16 {
        if y >= max_y {
            return y;
        }
        let mut meta: Vec<Span<'_>> = Vec::new();
        if !effect.author.is_empty() {
            meta.push(Span::styled(&effect.author, Style::default().fg(CORAL)));
        }
        if !effect.category.is_empty() {
            if !meta.is_empty() {
                meta.push(Span::styled(" \u{00B7} ", Style::default().fg(DIM_GRAY)));
            }
            meta.push(Span::styled(
                &effect.category,
                Style::default().fg(DIM_GRAY),
            ));
        }
        if meta.is_empty() {
            return y;
        }
        frame.render_widget(Paragraph::new(Line::from(meta)), Rect::new(x, y, w, 1));
        y + 1
    }

    /// Thin horizontal separator line.
    fn render_separator(frame: &mut Frame, x: u16, y: u16, max_y: u16, w: u16) -> u16 {
        if y >= max_y {
            return y;
        }
        let sep: String = "\u{2500}".repeat(usize::from(w));
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                sep,
                Style::default().fg(BORDER_DIM),
            ))),
            Rect::new(x, y, w, 1),
        );
        y + 1
    }

    /// Audio / control count / preset count badges (no tags).
    fn render_feature_badges(
        frame: &mut Frame,
        effect: &EffectSummary,
        x: u16,
        y: u16,
        max_y: u16,
        w: u16,
    ) -> u16 {
        if y >= max_y {
            return y;
        }
        let mut badges: Vec<Span<'_>> = Vec::new();
        if effect.audio_reactive {
            badges.push(Span::styled(
                "\u{266B} Audio",
                Style::default().fg(NEON_CYAN),
            ));
        }
        if !effect.controls.is_empty() {
            if !badges.is_empty() {
                badges.push(Span::styled("  ", Style::default()));
            }
            badges.push(Span::styled(
                format!("\u{2699} {} ctrl", effect.controls.len()),
                Style::default().fg(ELECTRIC_YELLOW),
            ));
        }
        if !effect.presets.is_empty() {
            if !badges.is_empty() {
                badges.push(Span::styled("  ", Style::default()));
            }
            badges.push(Span::styled(
                format!("\u{2605} {} pre", effect.presets.len()),
                Style::default().fg(CORAL),
            ));
        }
        if badges.is_empty() {
            return y;
        }
        frame.render_widget(Paragraph::new(Line::from(badges)), Rect::new(x, y, w, 1));
        y + 1
    }

    /// Tag list on its own line, natively wrapped, capped at 2 rows.
    fn render_tags(
        frame: &mut Frame,
        effect: &EffectSummary,
        x: u16,
        y: u16,
        max_y: u16,
        w: u16,
    ) -> u16 {
        if effect.tags.is_empty() || y >= max_y {
            return y;
        }
        let max_h = (max_y - y).min(2);
        if max_h == 0 {
            return y;
        }
        let tag_str = effect.tags.join("  ");
        frame.render_widget(
            Paragraph::new(Span::styled(tag_str, Style::default().fg(DIM_GRAY)))
                .wrap(Wrap { trim: false }),
            Rect::new(x, y, w, max_h),
        );
        y + max_h
    }

    /// Render brightness gauge and FPS indicator.
    fn render_daemon_gauges(&self, frame: &mut Frame, x: u16, y: u16, max_y: u16, w: u16) {
        let Some(ds) = &self.daemon_state else { return };
        let mut y = y;

        if y < max_y {
            let bright_norm = f32::from(ds.brightness) / 100.0;
            let slider = ParamSlider::new("Bright", bright_norm).accent_color(ELECTRIC_PURPLE);
            frame.render_widget(slider, Rect::new(x, y, w, 1));
            y += 1;
        }

        if y < max_y {
            let fps_color = if ds.fps_actual >= ds.fps_target * 0.9 {
                SUCCESS_GREEN
            } else if ds.fps_actual >= ds.fps_target * 0.5 {
                ELECTRIC_YELLOW
            } else {
                ERROR_RED
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("FPS     ", Style::default().fg(DIM_GRAY)),
                    Span::styled(
                        format!("{:.0}", ds.fps_actual),
                        Style::default().fg(fps_color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" / {:.0}", ds.fps_target),
                        Style::default().fg(DIM_GRAY),
                    ),
                ])),
                Rect::new(x, y, w, 1),
            );
        }
    }

    /// Render idle state when no effect is active.
    fn render_idle_state(&self, frame: &mut Frame, area: Rect) {
        let mut lines: Vec<Line<'_>> = Vec::new();

        if let Some(ds) = &self.daemon_state {
            // Has daemon state — running but no effect active
            let status_color = if ds.running { SUCCESS_GREEN } else { ERROR_RED };
            let status_text = if ds.running { "running" } else { "stopped" };
            lines.push(Line::from(vec![
                Span::styled("\u{25CF} ", Style::default().fg(status_color)),
                Span::styled(
                    format!("Daemon {status_text}"),
                    Style::default().fg(status_color),
                ),
            ]));
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                "No effect active",
                Style::default().fg(DIM_GRAY),
            )));
            lines.push(Line::from(Span::styled(
                "Press [e] to browse effects",
                Style::default().fg(DIM_GRAY),
            )));
        } else {
            // No daemon state — show connection status
            match self.connection_status {
                ConnectionStatus::Connecting | ConnectionStatus::Reconnecting => {
                    lines.push(Line::from(vec![
                        Span::styled("\u{25CF} ", Style::default().fg(ELECTRIC_YELLOW)),
                        Span::styled("Connecting", Style::default().fg(ELECTRIC_YELLOW)),
                    ]));
                    lines.push(Line::raw(""));
                    lines.push(Line::from(Span::styled(
                        "Trying to reach daemon\u{2026}",
                        Style::default().fg(DIM_GRAY),
                    )));
                }
                _ => {
                    lines.push(Line::from(vec![
                        Span::styled("\u{25CF} ", Style::default().fg(ERROR_RED)),
                        Span::styled("Disconnected", Style::default().fg(ERROR_RED)),
                    ]));
                    lines.push(Line::raw(""));
                    if let Some(reason) = &self.disconnect_reason {
                        lines.push(Line::from(Span::styled(
                            truncate_str(reason, area.width.saturating_sub(2).into()),
                            Style::default().fg(DIM_GRAY),
                        )));
                    } else {
                        lines.push(Line::from(Span::styled(
                            "Waiting for daemon\u{2026}",
                            Style::default().fg(DIM_GRAY),
                        )));
                    }
                }
            }
        }

        frame.render_widget(Paragraph::new(lines), area);
    }

    /// Render the canvas preview panel.
    ///
    /// Renders the bordered block and a placeholder when no canvas data is
    /// available. When data IS available, the actual pixel rendering is done
    /// by App as an overlay using the live ratatui-image protocol — see
    /// `Component::canvas_preview_area()` and `App::render`.
    fn render_preview_panel(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(Span::styled(
                " Preview ",
                Style::default()
                    .fg(ELECTRIC_PURPLE)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BORDER_DIM));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width < 2 || inner.height < 1 {
            self.preview_inner.set(None);
            return;
        }

        if let Some(cf) = &self.canvas_frame {
            let _ = cf;
            self.preview_inner.set(Some(inner));
        } else {
            // No data yet — clear the cached rect and show a centered hint.
            self.preview_inner.set(None);
            let placeholder = Paragraph::new(Line::from(Span::styled(
                "No canvas data",
                Style::default().fg(Color::Rgb(50, 50, 70)),
            )));
            let y = inner.y + inner.height / 2;
            let x = inner.x + inner.width.saturating_sub(14) / 2;
            frame.render_widget(placeholder, Rect::new(x, y, 14, 1));
        }
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

    fn handle_mouse_event(&mut self, mouse: MouseEvent) -> Result<Option<Action>> {
        // Resizable splits take priority
        if self.v_split.handle_mouse(&mouse) || self.h_split.handle_mouse(&mouse) {
            return Ok(Some(Action::Render));
        }

        let col = mouse.column;
        let row = mouse.row;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(action) = self.quick_action_at(col, row) {
                    return Ok(Some(action));
                }

                if let Some(index) = self.device_index_at(col, row)
                    && index != self.selected_device
                {
                    self.selected_device = index;
                    return Ok(Some(Action::Render));
                }
            }
            MouseEventKind::ScrollDown if rect_contains(self.devices_rect.get(), col, row) => {
                if !self.devices.is_empty() {
                    self.selected_device =
                        (self.selected_device + 1).min(self.devices.len().saturating_sub(1));
                    return Ok(Some(Action::Render));
                }
            }
            MouseEventKind::ScrollUp if rect_contains(self.devices_rect.get(), col, row) => {
                if !self.devices.is_empty() {
                    self.selected_device = self.selected_device.saturating_sub(1);
                    return Ok(Some(Action::Render));
                }
            }
            _ => {}
        }

        Ok(None)
    }

    fn update(&mut self, action: &Action) -> Result<Option<Action>> {
        match action {
            Action::DaemonStateUpdated(state) | Action::DaemonConnected(state) => {
                self.daemon_state = Some(*state.clone());
                self.connection_status = ConnectionStatus::Connected;
                self.disconnect_reason = None;
            }
            Action::DaemonDisconnected(reason) => {
                self.daemon_state = None;
                self.canvas_frame = None;
                self.connection_status = ConnectionStatus::Disconnected;
                self.disconnect_reason = Some(reason.clone());
            }
            Action::DaemonReconnecting => {
                self.connection_status = ConnectionStatus::Reconnecting;
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
            Action::CanvasFrameReceived(frame) => {
                self.canvas_frame = Some(CanvasPreviewState::from(frame.as_ref()));
            }
            _ => {}
        }
        Ok(None)
    }

    fn render(&self, frame: &mut Frame, area: Rect) {
        if area.width < 4 || area.height < 4 {
            return;
        }

        // Quick actions bar stays fixed at 3 rows; rest is resizable
        let actions_h = 3u16.min(area.height);
        let main_area = Rect::new(
            area.x,
            area.y,
            area.width,
            area.height.saturating_sub(actions_h),
        );
        let actions_area = Rect::new(area.x, area.y + main_area.height, area.width, actions_h);

        // Vertical split: top section / devices table
        let [top_area, devices_area] = self.v_split.layout(main_area);

        // Horizontal split: effect panel / preview panel
        let [effect_area, preview_area] = self.h_split.layout(top_area);

        self.devices_rect.set(devices_area);
        self.actions_rect.set(actions_area);

        self.render_effect_panel(frame, effect_area);
        self.render_preview_panel(frame, preview_area);
        self.render_devices_table(frame, devices_area);
        self.render_quick_actions(frame, actions_area);

        // Split divider overlays
        self.v_split.render_divider(frame);
        self.h_split.render_divider(frame);
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

    fn canvas_preview_area(&self) -> Option<Rect> {
        self.preview_inner.get()
    }
}

impl DashboardView {
    fn quick_action_at(&self, col: u16, row: u16) -> Option<Action> {
        let area = self.actions_rect.get();
        if !rect_contains(area, col, row) || row != area.y.saturating_add(1) {
            return None;
        }

        let mut cursor = area.x.saturating_add(2);
        for fav_id in self.favorites.iter().take(5) {
            let label_width = u16::try_from(4 + self.favorite_name(fav_id).chars().count()).ok()?;
            if col >= cursor && col < cursor.saturating_add(label_width) {
                return Some(Action::ApplyEffect(fav_id.clone()));
            }
            cursor = cursor.saturating_add(label_width).saturating_add(2);
        }
        None
    }

    fn device_index_at(&self, col: u16, row: u16) -> Option<usize> {
        let area = self.devices_rect.get();
        if !rect_contains(area, col, row) {
            return None;
        }

        let first_row = area.y.saturating_add(2);
        if row < first_row {
            return None;
        }
        let index = usize::from(row - first_row);
        (index < self.devices.len()).then_some(index)
    }
}

/// Truncate a string to fit within `max_len` characters, appending `…` if needed.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len > 1 {
        format!("{}\u{2026}", &s[..max_len - 1])
    } else {
        "\u{2026}".to_string()
    }
}

fn rect_contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}
