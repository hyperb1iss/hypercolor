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
use crate::widgets::{ColorPickerPopup, ParamSlider, hsl_to_rgb, rgb_to_hsl};

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

/// Color picker popup state.
struct ColorPickerState {
    /// Index of the control being edited.
    ctrl_idx: usize,
    /// HSL channels: [hue 0-360, saturation 0-1, lightness 0-1].
    hsl: [f32; 3],
    /// Currently selected channel (0=H, 1=S, 2=L).
    selected_channel: usize,
}

/// Focused parameter editing view for the active effect.
pub struct EffectControlView {
    focused: bool,
    action_tx: Option<UnboundedSender<Action>>,

    // Effect catalog (kept in sync via broadcast)
    all_effects: Vec<EffectSummary>,

    // Active effect data
    effect_id: String,
    effect_name: String,
    effect_description: String,
    controls: Vec<ControlDefinition>,
    control_values: HashMap<String, ControlValue>,

    // Navigation
    selected_control: usize,

    // Color picker popup
    color_picker: Option<ColorPickerState>,
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
            all_effects: Vec::new(),
            effect_id: String::new(),
            effect_name: String::new(),
            effect_description: String::new(),
            controls: Vec::new(),
            control_values: HashMap::new(),
            selected_control: 0,
            color_picker: None,
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

    /// Look up the current `effect_id` in the cached catalog and load it.
    /// Returns `true` if the effect was found and loaded.
    fn try_load_from_catalog(&mut self) -> bool {
        if let Some(effect) = self
            .all_effects
            .iter()
            .find(|e| e.id == self.effect_id)
            .cloned()
        {
            self.load_effect(&effect);
            true
        } else {
            false
        }
    }

    /// Populate controls from an effect summary.
    fn load_effect(&mut self, effect: &EffectSummary) {
        self.effect_id.clone_from(&effect.id);
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

    /// Compute the maximum label width across all controls for alignment.
    fn max_label_width(&self) -> usize {
        self.controls
            .iter()
            .map(|c| c.name.chars().count())
            .max()
            .unwrap_or(0)
            .max(4) // minimum 4 chars
    }

    /// Render all controls starting from the given y position.
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    fn render_controls(&self, frame: &mut Frame, inner: Rect, start_y: u16) {
        let ctrl_width = inner.width.saturating_sub(4);
        let label_w = self.max_label_width();
        let mut content_y = start_y;

        for (i, ctrl) in self.controls.iter().enumerate() {
            if content_y >= inner.y + inner.height {
                break;
            }

            let is_selected = i == self.selected_control;

            let available = (inner.y + inner.height).saturating_sub(content_y);
            if available < 1 {
                break;
            }

            let ctrl_area = Rect::new(inner.x + 2, content_y, ctrl_width, available.min(2));

            // Selection indicator — glowing bar
            if is_selected {
                frame.render_widget(
                    Paragraph::new(Span::styled("\u{2503}", Style::default().fg(NEON_CYAN))),
                    Rect::new(inner.x + 1, content_y, 1, 1),
                );
            }

            self.render_single_control(frame, ctrl, ctrl_area, is_selected, label_w);

            // Spacing: 2 rows per control (1 content + 1 gap)
            content_y += 2;
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
                    Span::styled("Enter", Style::default().fg(ELECTRIC_YELLOW)),
                    Span::styled(" edit color", Style::default().fg(DIM_GRAY)),
                ])),
                Rect::new(inner.x + 2, hint_y, inner.width.saturating_sub(4), 1),
            );
        }
    }

    /// Dispatch rendering for a single control by type.
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    fn render_single_control(
        &self,
        frame: &mut Frame,
        ctrl: &ControlDefinition,
        area: Rect,
        is_selected: bool,
        label_w: usize,
    ) {
        match ctrl.control_type.as_str() {
            "slider" => self.render_slider_control(frame, ctrl, area, is_selected, label_w),
            "toggle" => self.render_toggle_control(frame, ctrl, area, is_selected, label_w),
            "dropdown" => self.render_dropdown_control(frame, ctrl, area, is_selected, label_w),
            "color" => self.render_color_control(frame, ctrl, area, is_selected, label_w),
            _ => Self::render_unknown_control(frame, ctrl, area, label_w),
        }
    }

    /// Render a slider — single-line with label, gradient bar, and value.
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    fn render_slider_control(
        &self,
        frame: &mut Frame,
        ctrl: &ControlDefinition,
        area: Rect,
        is_selected: bool,
        label_w: usize,
    ) {
        if area.width < 10 {
            return;
        }

        let norm = self.normalized_value(ctrl);

        // Per-type gradient colors: different control vibes
        let (grad_from, grad_to) = if is_selected {
            ((128, 255, 234), (225, 53, 255)) // Neon Cyan → Electric Purple
        } else {
            ((255, 106, 193), (225, 53, 255)) // Coral → Electric Purple
        };

        let padded_label = format!("{:<width$}", ctrl.name, width = label_w + 1);
        let slider = ParamSlider::new(&padded_label, norm)
            .gradient_fill(grad_from, grad_to)
            .accent_color(NEON_CYAN);
        frame.render_widget(slider, Rect::new(area.x, area.y, area.width, 1));
    }

    /// Render a toggle — inline label + styled switch.
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    fn render_toggle_control(
        &self,
        frame: &mut Frame,
        ctrl: &ControlDefinition,
        area: Rect,
        is_selected: bool,
        label_w: usize,
    ) {
        if area.width < 10 {
            return;
        }

        let on = self.current_value(ctrl).as_bool().unwrap_or(false);
        let name_style = selected_style(is_selected);

        let (indicator, state_text, state_color) = if on {
            ("\u{25C9}", "On", SUCCESS_GREEN) // ◉
        } else {
            ("\u{25CB}", "Off", DIM_GRAY) // ○
        };

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("{:<width$}", ctrl.name, width = label_w + 1),
                    name_style,
                ),
                Span::styled(
                    format!("{indicator} "),
                    Style::default().fg(state_color),
                ),
                Span::styled(state_text, Style::default().fg(state_color)),
            ])),
            Rect::new(area.x, area.y, area.width, 1),
        );
    }

    /// Render a dropdown — label + current value with arrows.
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    fn render_dropdown_control(
        &self,
        frame: &mut Frame,
        ctrl: &ControlDefinition,
        area: Rect,
        is_selected: bool,
        label_w: usize,
    ) {
        if area.width < 10 {
            return;
        }

        let current_text = match self.current_value(ctrl) {
            ControlValue::Text(s) => s,
            _ => String::new(),
        };

        let name_style = selected_style(is_selected);
        let value_color = if is_selected {
            NEON_CYAN
        } else {
            ELECTRIC_YELLOW
        };

        let arrows = if is_selected {
            "\u{25C0} \u{25B6}"
        } else {
            ""
        };

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("{:<width$}", ctrl.name, width = label_w + 1),
                    name_style,
                ),
                Span::styled(
                    format!("\u{25B8} {current_text}"),
                    Style::default().fg(value_color),
                ),
                Span::styled(format!("  {arrows}"), Style::default().fg(DIM_GRAY)),
            ])),
            Rect::new(area.x, area.y, area.width, 1),
        );
    }

    /// Render a color swatch — label + swatch blocks + hex + edit hint.
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    fn render_color_control(
        &self,
        frame: &mut Frame,
        ctrl: &ControlDefinition,
        area: Rect,
        is_selected: bool,
        label_w: usize,
    ) {
        if area.width < 10 {
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

        let mut spans = vec![
            Span::styled(
                format!("{:<width$}", ctrl.name, width = label_w + 1),
                name_style,
            ),
            Span::styled(
                "\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}",
                Style::default().fg(swatch_color),
            ),
            Span::styled(
                format!("  #{r:02x}{g:02x}{b:02x}"),
                Style::default()
                    .fg(BASE_WHITE)
                    .add_modifier(Modifier::BOLD),
            ),
        ];

        if is_selected {
            spans.push(Span::styled(
                "  \u{25C8}Enter",
                Style::default().fg(ELECTRIC_PURPLE),
            ));
        }

        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(area.x, area.y, area.width, 1),
        );
    }

    /// Render an unknown control type as plain text.
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    fn render_unknown_control(
        frame: &mut Frame,
        ctrl: &ControlDefinition,
        area: Rect,
        label_w: usize,
    ) {
        let ctrl_type = &ctrl.control_type;
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("{:<width$}", ctrl.name, width = label_w + 1),
                    Style::default().fg(BASE_WHITE),
                ),
                Span::styled(format!("({ctrl_type})"), Style::default().fg(DIM_GRAY)),
            ])),
            Rect::new(area.x, area.y, area.width, 1),
        );
    }

    /// Open the color picker for the currently selected control.
    fn open_color_picker(&mut self) {
        let idx = self.selected_control;
        if let Some(ctrl) = self.controls.get(idx)
            && ctrl.control_type == "color"
        {
            let color_arr = match self.current_value(ctrl) {
                ControlValue::Color(c) => c,
                _ => [0.5, 0.5, 0.5, 1.0],
            };
            let (h, s, l) = rgb_to_hsl(color_arr[0], color_arr[1], color_arr[2]);
            self.color_picker = Some(ColorPickerState {
                ctrl_idx: idx,
                hsl: [h, s, l],
                selected_channel: 0,
            });
        }
    }

    /// Confirm the color picker and apply the selected color.
    fn confirm_color_picker(&mut self) -> Option<Action> {
        let picker = self.color_picker.take()?;
        let ctrl = self.controls.get(picker.ctrl_idx)?;
        let (r, g, b) = hsl_to_rgb(picker.hsl[0], picker.hsl[1], picker.hsl[2]);
        let old = match self.current_value(ctrl) {
            ControlValue::Color(c) => c[3],
            _ => 1.0,
        };
        let new_cv = ControlValue::Color([r, g, b, old]);
        self.control_values.insert(ctrl.id.clone(), new_cv.clone());
        Some(Action::UpdateControl(ctrl.id.clone(), new_cv))
    }

    /// Handle key events while the color picker is open.
    fn handle_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        let picker = self.color_picker.as_mut()?;

        match key.code {
            KeyCode::Esc => {
                self.color_picker = None;
                None
            }
            KeyCode::Enter => self.confirm_color_picker(),
            KeyCode::Char('j') | KeyCode::Down => {
                picker.selected_channel = (picker.selected_channel + 1).min(2);
                None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                picker.selected_channel = picker.selected_channel.saturating_sub(1);
                None
            }
            KeyCode::Char('h') | KeyCode::Left => {
                let step = match picker.selected_channel {
                    0 => -5.0,
                    _ => -0.02,
                };
                adjust_hsl_channel(&mut picker.hsl, picker.selected_channel, step);
                None
            }
            KeyCode::Char('l') | KeyCode::Right => {
                let step = match picker.selected_channel {
                    0 => 5.0,
                    _ => 0.02,
                };
                adjust_hsl_channel(&mut picker.hsl, picker.selected_channel, step);
                None
            }
            _ => None,
        }
    }

    /// Render the color picker popup overlay.
    fn render_color_picker(&self, frame: &mut Frame, area: Rect) {
        if let Some(ref picker) = self.color_picker {
            let ctrl_name = self
                .controls
                .get(picker.ctrl_idx)
                .map_or("Color", |c| &c.name);

            // Center popup: 40 wide, 9 tall
            let popup_w = 40.min(area.width.saturating_sub(4));
            let popup_h = 9.min(area.height.saturating_sub(2));
            let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
            let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
            let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

            let widget = ColorPickerPopup::new(ctrl_name, picker.hsl, picker.selected_channel);
            frame.render_widget(widget, popup_area);
        }
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

/// Adjust an HSL channel in-place, clamping to valid ranges.
fn adjust_hsl_channel(hsl: &mut [f32; 3], channel: usize, delta: f32) {
    match channel {
        0 => hsl[0] = (hsl[0] + delta).rem_euclid(360.0),
        1 => hsl[1] = (hsl[1] + delta).clamp(0.0, 1.0),
        _ => hsl[2] = (hsl[2] + delta).clamp(0.0, 1.0),
    }
}

impl Component for EffectControlView {
    fn init(&mut self, action_tx: UnboundedSender<Action>) -> Result<()> {
        self.action_tx = Some(action_tx);
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        // Color picker intercepts all keys when open
        if self.color_picker.is_some() {
            return Ok(self.handle_picker_key(key));
        }

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

            // Toggle / activate / open color picker
            KeyCode::Char(' ') | KeyCode::Enter => {
                let idx = self.selected_control;
                let ctrl_type = self.controls.get(idx).map(|c| c.control_type.clone());
                let action = match ctrl_type.as_deref() {
                    Some("toggle") => self.toggle_bool(idx),
                    Some("dropdown") => self.cycle_dropdown(idx, true),
                    Some("color") => {
                        self.open_color_picker();
                        None
                    }
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
                self.all_effects.clone_from(effects);
                // If we have a pending effect_id, try to resolve it now.
                if !self.effect_id.is_empty() && self.controls.is_empty() {
                    self.try_load_from_catalog();
                }
            }
            Action::DaemonStateUpdated(state) | Action::DaemonConnected(state) => {
                if let Some(effect_id) = &state.effect_id
                    && effect_id != &self.effect_id
                {
                    self.effect_id.clone_from(effect_id);
                    if let Some(name) = &state.effect_name {
                        self.effect_name.clone_from(name);
                    }
                    // Try to load full effect data from cached catalog.
                    if !self.try_load_from_catalog() {
                        self.control_values.clear();
                    }
                }
            }
            Action::ApplyEffect(_) => {
                self.effect_id.clear();
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

        // Color picker overlay (rendered last, on top)
        if self.color_picker.is_some() {
            self.render_color_picker(frame, area);
        }
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
