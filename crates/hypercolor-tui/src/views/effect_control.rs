//! Effect control view — focused parameter editing for the active effect.

use std::collections::HashMap;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use tokio::sync::mpsc::UnboundedSender;

use crate::action::Action;
use crate::component::Component;
use crate::state::{ControlDefinition, ControlValue, EffectSummary};
use crate::widgets::ParamSlider;

// ── SilkCircuit Neon palette ───────────────────────────────────────────

const NEON_CYAN: Color = Color::Rgb(128, 255, 234);
const ELECTRIC_PURPLE: Color = Color::Rgb(225, 53, 255);
const ELECTRIC_YELLOW: Color = Color::Rgb(241, 250, 140);
const SUCCESS_GREEN: Color = Color::Rgb(80, 250, 123);
const BASE_WHITE: Color = Color::Rgb(248, 248, 242);
const DIM_GRAY: Color = Color::Rgb(98, 114, 164);
const BORDER_DIM: Color = Color::Rgb(90, 21, 102);

/// Slider adjustment step when no `step` is defined on the control.
const DEFAULT_STEP: f32 = 0.05;

// ── Effect Control View ────────────────────────────────────────────────

/// Focused parameter editing view for the active effect.
pub struct EffectControlView {
    focused: bool,
    action_tx: Option<UnboundedSender<Action>>,

    // Effect data
    effect_name: String,
    effect_description: String,
    controls: Vec<ControlDefinition>,
    control_values: HashMap<String, ControlValue>,

    // Navigation
    selected_control: usize,
}

impl Default for EffectControlView {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectControlView {
    #[must_use]
    pub fn new() -> Self {
        Self {
            focused: false,
            action_tx: None,
            effect_name: String::new(),
            effect_description: String::new(),
            controls: Vec::new(),
            control_values: HashMap::new(),
            selected_control: 0,
        }
    }

    /// Get the current value for a control, falling back to default.
    fn current_value(&self, ctrl: &ControlDefinition) -> ControlValue {
        self.control_values
            .get(&ctrl.id)
            .cloned()
            .unwrap_or_else(|| ctrl.default_value.clone())
    }

    /// Normalize a numeric control value to `[0, 1]`.
    fn normalized_value(&self, ctrl: &ControlDefinition) -> f32 {
        normalize(
            self.current_value(ctrl).as_f32().unwrap_or(0.0),
            ctrl.min.unwrap_or(0.0),
            ctrl.max.unwrap_or(1.0),
        )
    }

    /// Adjust a slider control by the given delta direction (-1 or +1).
    fn adjust_slider(&mut self, ctrl_idx: usize, direction: f32) -> Option<Action> {
        let ctrl = self.controls.get(ctrl_idx)?;
        let current = self.current_value(ctrl).as_f32().unwrap_or(0.0);
        let step = ctrl.step.unwrap_or(DEFAULT_STEP);
        let min = ctrl.min.unwrap_or(0.0);
        let max = ctrl.max.unwrap_or(1.0);

        let new_val = (current + step * direction).clamp(min, max);
        let new_cv = ControlValue::Float(new_val);
        self.control_values.insert(ctrl.id.clone(), new_cv.clone());

        Some(Action::UpdateControl(ctrl.id.clone(), new_cv))
    }

    /// Toggle a boolean control.
    fn toggle_bool(&mut self, ctrl_idx: usize) -> Option<Action> {
        let ctrl = self.controls.get(ctrl_idx)?;
        let current = self.current_value(ctrl).as_bool().unwrap_or(false);
        let new_cv = ControlValue::Boolean(!current);
        self.control_values.insert(ctrl.id.clone(), new_cv.clone());

        Some(Action::UpdateControl(ctrl.id.clone(), new_cv))
    }

    /// Cycle a dropdown control to the next label.
    fn cycle_dropdown(&mut self, ctrl_idx: usize, forward: bool) -> Option<Action> {
        let ctrl = self.controls.get(ctrl_idx)?;
        if ctrl.labels.is_empty() {
            return None;
        }

        let current_text = match self.current_value(ctrl) {
            ControlValue::Text(s) => s,
            _ => String::new(),
        };

        let current_idx = ctrl
            .labels
            .iter()
            .position(|l| l == &current_text)
            .unwrap_or(0);

        let new_idx = if forward {
            (current_idx + 1) % ctrl.labels.len()
        } else if current_idx == 0 {
            ctrl.labels.len().saturating_sub(1)
        } else {
            current_idx - 1
        };

        let new_text = ctrl.labels.get(new_idx)?.clone();
        let new_cv = ControlValue::Text(new_text);
        self.control_values.insert(ctrl.id.clone(), new_cv.clone());

        Some(Action::UpdateControl(ctrl.id.clone(), new_cv))
    }

    /// Populate controls from an effect summary.
    fn load_effect(&mut self, effect: &EffectSummary) {
        self.effect_name.clone_from(&effect.name);
        self.effect_description.clone_from(&effect.description);
        self.controls.clone_from(&effect.controls);
        self.control_values.clear();
        for ctrl in &self.controls {
            self.control_values
                .insert(ctrl.id.clone(), ctrl.default_value.clone());
        }
        self.selected_control = 0;
    }

    // ── Rendering helpers ───────────────────────────────────────────

    /// Render the description at the top; returns the next y position.
    fn render_description(&self, frame: &mut Frame, inner: Rect) -> u16 {
        let mut content_y = inner.y;
        if !self.effect_description.is_empty() {
            let desc_h = 2.min(inner.height);
            frame.render_widget(
                Paragraph::new(self.effect_description.as_str())
                    .style(Style::default().fg(DIM_GRAY))
                    .wrap(Wrap { trim: true }),
                Rect::new(
                    inner.x + 2,
                    content_y,
                    inner.width.saturating_sub(4),
                    desc_h,
                ),
            );
            content_y += desc_h + 1;
        }
        content_y
    }

    /// Render all controls starting from the given y position.
    fn render_controls(&self, frame: &mut Frame, inner: Rect, start_y: u16) {
        let ctrl_width = inner.width.saturating_sub(4);
        let mut content_y = start_y;

        for (i, ctrl) in self.controls.iter().enumerate() {
            if content_y >= inner.y + inner.height {
                break;
            }

            let is_selected = i == self.selected_control;
            let ctrl_type = ctrl.control_type.as_str();

            let row_height: u16 = match ctrl_type {
                "slider" => 3,
                "dropdown" => 2,
                _ => 1,
            };

            let available = (inner.y + inner.height).saturating_sub(content_y);
            if available < 1 {
                break;
            }

            let actual_height = row_height.min(available);
            let ctrl_area = Rect::new(inner.x + 2, content_y, ctrl_width, actual_height);

            // Selection indicator
            if is_selected {
                frame.render_widget(
                    Paragraph::new(Span::styled("\u{25B8}", Style::default().fg(NEON_CYAN))),
                    Rect::new(inner.x, content_y, 2, 1),
                );
            }

            self.render_single_control(frame, ctrl, ctrl_area, is_selected);
            content_y += row_height + 1;
        }

        // Key hints at the bottom
        let hint_y = inner.y + inner.height.saturating_sub(1);
        if hint_y > content_y {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("j/k", Style::default().fg(ELECTRIC_YELLOW)),
                    Span::styled(" navigate  ", Style::default().fg(DIM_GRAY)),
                    Span::styled("h/l", Style::default().fg(ELECTRIC_YELLOW)),
                    Span::styled(" adjust  ", Style::default().fg(DIM_GRAY)),
                    Span::styled("Space", Style::default().fg(ELECTRIC_YELLOW)),
                    Span::styled(" toggle", Style::default().fg(DIM_GRAY)),
                ])),
                Rect::new(inner.x + 2, hint_y, inner.width.saturating_sub(4), 1),
            );
        }
    }

    /// Dispatch rendering for a single control by type.
    fn render_single_control(
        &self,
        frame: &mut Frame,
        ctrl: &ControlDefinition,
        area: Rect,
        is_selected: bool,
    ) {
        match ctrl.control_type.as_str() {
            "slider" => self.render_slider_control(frame, ctrl, area, is_selected),
            "toggle" => self.render_toggle_control(frame, ctrl, area, is_selected),
            "dropdown" => self.render_dropdown_control(frame, ctrl, area, is_selected),
            "color" => self.render_color_control(frame, ctrl, area, is_selected),
            _ => Self::render_unknown_control(frame, ctrl, area),
        }
    }

    /// Render a slider control row.
    fn render_slider_control(
        &self,
        frame: &mut Frame,
        ctrl: &ControlDefinition,
        area: Rect,
        is_selected: bool,
    ) {
        if area.height < 2 || area.width < 10 {
            return;
        }

        let name_style = selected_style(is_selected);

        // Control name
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(&ctrl.name, name_style))),
            Rect::new(area.x, area.y, area.width, 1),
        );

        // Slider bar
        let norm = self.normalized_value(ctrl);
        let accent = if is_selected {
            NEON_CYAN
        } else {
            Color::Rgb(255, 106, 193)
        };
        let arrow_prefix = if is_selected { "\u{25C0} " } else { "  " };
        let slider = ParamSlider::new(arrow_prefix, norm).accent_color(accent);
        frame.render_widget(slider, Rect::new(area.x, area.y + 1, area.width, 1));
    }

    /// Render a toggle control row.
    fn render_toggle_control(
        &self,
        frame: &mut Frame,
        ctrl: &ControlDefinition,
        area: Rect,
        is_selected: bool,
    ) {
        if area.height < 1 || area.width < 10 {
            return;
        }

        let on = self.current_value(ctrl).as_bool().unwrap_or(false);
        let name_style = selected_style(is_selected);
        let (toggle_text, toggle_color) = if on {
            ("[\u{25CF}] On", SUCCESS_GREEN)
        } else {
            ("[ ] Off", DIM_GRAY)
        };

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{:<20}", ctrl.name), name_style),
                Span::styled(toggle_text, Style::default().fg(toggle_color)),
            ])),
            Rect::new(area.x, area.y, area.width, 1),
        );
    }

    /// Render a dropdown control row.
    fn render_dropdown_control(
        &self,
        frame: &mut Frame,
        ctrl: &ControlDefinition,
        area: Rect,
        is_selected: bool,
    ) {
        if area.height < 1 || area.width < 10 {
            return;
        }

        let current_text = match self.current_value(ctrl) {
            ControlValue::Text(s) => s,
            _ => String::new(),
        };

        let name_style = selected_style(is_selected);
        let value_style = if is_selected {
            Style::default().fg(NEON_CYAN)
        } else {
            Style::default().fg(ELECTRIC_YELLOW)
        };

        let arrows = if is_selected { "\u{25C0} \u{25B6}" } else { "" };

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{:<20}", ctrl.name), name_style),
                Span::styled(format!("[\u{25B8} {current_text}]"), value_style),
                Span::styled(format!("  {arrows}"), Style::default().fg(DIM_GRAY)),
            ])),
            Rect::new(area.x, area.y, area.width, 1),
        );

        // Labels on second line if space permits
        if area.height >= 2 {
            let labels_text = ctrl.labels.join(" \u{00B7} ");
            if !labels_text.is_empty() {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        format!("{:>20}{labels_text}", ""),
                        Style::default().fg(DIM_GRAY),
                    ))),
                    Rect::new(area.x, area.y + 1, area.width, 1),
                );
            }
        }
    }

    /// Render a color swatch control row.
    fn render_color_control(
        &self,
        frame: &mut Frame,
        ctrl: &ControlDefinition,
        area: Rect,
        is_selected: bool,
    ) {
        if area.height < 1 || area.width < 10 {
            return;
        }

        let name_style = selected_style(is_selected);

        let color_arr = match self.current_value(ctrl) {
            ControlValue::Color(c) => c,
            _ => [0.5, 0.5, 0.5, 1.0],
        };

        let r = float_to_u8(color_arr[0]);
        let g = float_to_u8(color_arr[1]);
        let b = float_to_u8(color_arr[2]);
        let swatch_color = Color::Rgb(r, g, b);

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{:<20}", ctrl.name), name_style),
                Span::styled(
                    "\u{2588}\u{2588}\u{2588}\u{2588}",
                    Style::default().fg(swatch_color),
                ),
                Span::styled(
                    format!("  #{r:02x}{g:02x}{b:02x}"),
                    Style::default().fg(DIM_GRAY),
                ),
            ])),
            Rect::new(area.x, area.y, area.width, 1),
        );
    }

    /// Render an unknown control type as plain text.
    fn render_unknown_control(frame: &mut Frame, ctrl: &ControlDefinition, area: Rect) {
        let ctrl_type = &ctrl.control_type;
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("{:<20}", ctrl.name),
                    Style::default().fg(BASE_WHITE),
                ),
                Span::styled(format!("({ctrl_type})"), Style::default().fg(DIM_GRAY)),
            ])),
            Rect::new(area.x, area.y, area.width, 1),
        );
    }
}

/// Normalize a raw value to `[0, 1]` given min/max bounds.
fn normalize(raw: f32, min: f32, max: f32) -> f32 {
    if (max - min).abs() < f32::EPSILON {
        0.0
    } else {
        ((raw - min) / (max - min)).clamp(0.0, 1.0)
    }
}

/// Convert a `[0.0, 1.0]` float to a `u8` in `[0, 255]`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn float_to_u8(value: f32) -> u8 {
    value.mul_add(255.0, 0.5).clamp(0.0, 255.0) as u8
}

/// Build a style for a control label, highlighted when selected.
fn selected_style(is_selected: bool) -> Style {
    if is_selected {
        Style::default().fg(NEON_CYAN).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(BASE_WHITE)
    }
}

impl Component for EffectControlView {
    fn init(&mut self, action_tx: UnboundedSender<Action>) -> Result<()> {
        self.action_tx = Some(action_tx);
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        if self.controls.is_empty() {
            return Ok(None);
        }

        match key.code {
            // Navigate controls
            KeyCode::Char('j') | KeyCode::Down => {
                self.selected_control =
                    (self.selected_control + 1).min(self.controls.len().saturating_sub(1));
                Ok(None)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected_control = self.selected_control.saturating_sub(1);
                Ok(None)
            }

            // Adjust value left
            KeyCode::Char('h') | KeyCode::Left => {
                let idx = self.selected_control;
                let ctrl_type = self.controls.get(idx).map(|c| c.control_type.clone());
                let action = match ctrl_type.as_deref() {
                    Some("slider") => self.adjust_slider(idx, -1.0),
                    Some("dropdown") => self.cycle_dropdown(idx, false),
                    _ => None,
                };
                Ok(action)
            }

            // Adjust value right
            KeyCode::Char('l') | KeyCode::Right => {
                let idx = self.selected_control;
                let ctrl_type = self.controls.get(idx).map(|c| c.control_type.clone());
                let action = match ctrl_type.as_deref() {
                    Some("slider") => self.adjust_slider(idx, 1.0),
                    Some("dropdown") => self.cycle_dropdown(idx, true),
                    _ => None,
                };
                Ok(action)
            }

            // Toggle / activate
            KeyCode::Char(' ') | KeyCode::Enter => {
                let idx = self.selected_control;
                let ctrl_type = self.controls.get(idx).map(|c| c.control_type.clone());
                let action = match ctrl_type.as_deref() {
                    Some("toggle") => self.toggle_bool(idx),
                    Some("dropdown") => self.cycle_dropdown(idx, true),
                    _ => None,
                };
                Ok(action)
            }

            // Jump to top/bottom
            KeyCode::Char('g') => {
                self.selected_control = 0;
                Ok(None)
            }
            KeyCode::Char('G') => {
                self.selected_control = self.controls.len().saturating_sub(1);
                Ok(None)
            }

            _ => Ok(None),
        }
    }

    fn update(&mut self, action: &Action) -> Result<Option<Action>> {
        match action {
            Action::EffectsUpdated(effects) => {
                if let Some(effect) = effects.iter().find(|e| e.name == self.effect_name) {
                    self.load_effect(effect);
                }
            }
            Action::DaemonStateUpdated(state) | Action::DaemonConnected(state) => {
                if state.effect_id.is_some() {
                    let name_changed = state
                        .effect_name
                        .as_deref()
                        .is_some_and(|n| n != self.effect_name);
                    if name_changed {
                        if let Some(name) = &state.effect_name {
                            self.effect_name.clone_from(name);
                        }
                        self.control_values.clear();
                    }
                }
            }
            Action::ApplyEffect(_) => {
                self.effect_name.clear();
                self.control_values.clear();
                self.controls.clear();
                self.selected_control = 0;
            }
            _ => {}
        }
        Ok(None)
    }

    fn render(&self, frame: &mut Frame, area: Rect) {
        let title_text = if self.effect_name.is_empty() {
            "Effect Controls"
        } else {
            &self.effect_name
        };

        let block = Block::default()
            .title(Line::from(vec![
                Span::styled(" ", Style::default().fg(ELECTRIC_PURPLE)),
                Span::styled(
                    title_text,
                    Style::default()
                        .fg(ELECTRIC_PURPLE)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" \u{2500} Controls ", Style::default().fg(BORDER_DIM)),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BORDER_DIM));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width < 10 || inner.height < 4 {
            return;
        }

        if self.controls.is_empty() {
            let msg = if self.effect_name.is_empty() {
                "No effect active \u{2014} apply an effect from the browser (E)"
            } else {
                "This effect has no adjustable controls"
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(msg, Style::default().fg(DIM_GRAY)))),
                Rect::new(inner.x + 2, inner.y + 1, inner.width.saturating_sub(4), 1),
            );
            return;
        }

        let content_y = self.render_description(frame, inner);
        self.render_controls(frame, inner, content_y);
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn id(&self) -> &'static str {
        "effect_control"
    }
}
