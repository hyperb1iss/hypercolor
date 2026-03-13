//! Effect browser view — 3-pane layout: browse, preview, and control effects.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use tokio::sync::mpsc::UnboundedSender;

use crate::action::Action;
use crate::component::Component;
use crate::state::{ControlDefinition, ControlValue, EffectSummary};
use crate::widgets::{
    ColorPickerPopup, HalfBlockCanvas, ParamSlider, Split, SplitDirection, hsl_to_rgb, rgb_to_hsl,
};

// ── SilkCircuit Neon palette ───────────────────────────────────────────

const NEON_CYAN: Color = Color::Rgb(128, 255, 234);
const ELECTRIC_PURPLE: Color = Color::Rgb(225, 53, 255);
const ELECTRIC_YELLOW: Color = Color::Rgb(241, 250, 140);
const SUCCESS_GREEN: Color = Color::Rgb(80, 250, 123);
const BASE_WHITE: Color = Color::Rgb(248, 248, 242);
const DIM_GRAY: Color = Color::Rgb(98, 114, 164);
const BORDER_DIM: Color = Color::Rgb(90, 21, 102);

/// Slider adjustment step when no `step` is defined on the control.
const DEFAULT_STEP: f32 = 0.1;

/// Focus panel within the effect browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusPane {
    List,
    Preview,
    Controls,
}

/// Color picker popup state.
struct ColorPickerState {
    ctrl_idx: usize,
    hsl: [f32; 3],
    selected_channel: usize,
}

// ── Effect Browser View ────────────────────────────────────────────────

/// Three-pane effect browser: list (left), preview (top-right), controls (bottom-right).
pub struct EffectBrowserView {
    focused: bool,
    action_tx: Option<UnboundedSender<Action>>,
    focus_pane: FocusPane,

    // Data
    all_effects: Vec<EffectSummary>,
    effects: Vec<EffectSummary>,
    favorites: Vec<String>,
    selected_index: usize,
    scroll_offset: Cell<usize>,
    selected_preset: usize,

    // Search
    search_active: bool,
    search_query: String,

    // Canvas preview
    canvas_frame: Option<Arc<crate::state::CanvasFrame>>,

    // Control interaction state
    control_values: HashMap<String, ControlValue>,
    selected_control: usize,
    color_picker: Option<ColorPickerState>,

    // Slider acceleration state
    last_slider_adjust: Instant,
    slider_accel: f32,

    // Resizable panel splits
    h_split: Split,
    v_split: Split,

    // Layout cache for mouse hit-testing (populated during render)
    list_rect: Cell<Rect>,
    preview_rect: Cell<Rect>,
    controls_rect: Cell<Rect>,
    list_inner_rect: Cell<Rect>,
    controls_content_y: Cell<u16>,
    controls_inner_rect: Cell<Rect>,
    controls_label_w: Cell<usize>,
}

impl Default for EffectBrowserView {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectBrowserView {
    #[must_use]
    pub fn new() -> Self {
        Self {
            focused: false,
            action_tx: None,
            focus_pane: FocusPane::List,
            all_effects: Vec::new(),
            effects: Vec::new(),
            favorites: Vec::new(),
            selected_index: 0,
            scroll_offset: Cell::new(0),
            selected_preset: 0,
            search_active: false,
            search_query: String::new(),
            canvas_frame: None,
            control_values: HashMap::new(),
            selected_control: 0,
            color_picker: None,
            last_slider_adjust: Instant::now(),
            slider_accel: 1.0,
            h_split: Split::new(SplitDirection::Horizontal, 0.35).min_sizes(15, 25),
            v_split: Split::new(SplitDirection::Vertical, 0.45).min_sizes(5, 5),
            list_rect: Cell::new(Rect::default()),
            preview_rect: Cell::new(Rect::default()),
            controls_rect: Cell::new(Rect::default()),
            list_inner_rect: Cell::new(Rect::default()),
            controls_content_y: Cell::new(0),
            controls_inner_rect: Cell::new(Rect::default()),
            controls_label_w: Cell::new(0),
        }
    }

    // ── Filtering & selection ────────────────────────────────────────

    fn apply_filter(&mut self) {
        let query = self.search_query.to_lowercase();
        if query.is_empty() {
            self.effects.clone_from(&self.all_effects);
        } else {
            self.effects = self
                .all_effects
                .iter()
                .filter(|e| {
                    e.name.to_lowercase().contains(&query)
                        || e.category.to_lowercase().contains(&query)
                        || e.tags.iter().any(|t| t.to_lowercase().contains(&query))
                })
                .cloned()
                .collect();
        }
        self.clamp_selection();
        self.scroll_offset.set(0);
    }

    fn clamp_selection(&mut self) {
        if self.effects.is_empty() {
            self.selected_index = 0;
        } else {
            self.selected_index = self
                .selected_index
                .min(self.effects.len().saturating_sub(1));
        }
        self.selected_preset = 0;
    }

    fn is_favorite(&self, id: &str) -> bool {
        self.favorites.iter().any(|f| f == id)
    }

    fn selected_effect(&self) -> Option<&EffectSummary> {
        self.effects.get(self.selected_index)
    }

    /// Load control defaults for the selected effect.
    fn sync_controls_to_selection(&mut self) {
        self.control_values.clear();
        self.selected_control = 0;
        self.color_picker = None;
        if let Some(effect) = self.effects.get(self.selected_index) {
            let defaults: Vec<_> = effect
                .controls
                .iter()
                .map(|c| (c.id.clone(), c.default_value.clone()))
                .collect();
            for (id, val) in defaults {
                self.control_values.insert(id, val);
            }
        }
    }

    // ── Control value helpers ────────────────────────────────────────

    fn adjust_slider(&mut self, ctrl_idx: usize, direction: f32) -> Option<Action> {
        let ctrl = self
            .effects
            .get(self.selected_index)?
            .controls
            .get(ctrl_idx)?
            .clone();
        let current = current_value(&self.control_values, &ctrl)
            .as_f32()
            .unwrap_or(0.0);
        let step = ctrl.step.unwrap_or(DEFAULT_STEP);
        let min = ctrl.min.unwrap_or(0.0);
        let max = ctrl.max.unwrap_or(1.0);

        // Adaptive acceleration: ramp up when holding the key, reset on pause.
        // Max accel scales with how many steps span the range so sliders with
        // big ranges (e.g. [0,100] step 1) get fast traversal while small ranges
        // stay precise.
        let now = Instant::now();
        if now.duration_since(self.last_slider_adjust).as_millis() < 200 {
            let range = max - min;
            let steps_in_range = range / step;
            let max_accel = (steps_in_range / 30.0).max(1.0);
            let ramp = (max_accel - 1.0) / 15.0;
            self.slider_accel = (self.slider_accel + ramp).min(max_accel);
        } else {
            self.slider_accel = 1.0;
        }
        self.last_slider_adjust = now;

        let new_val = (current + step * self.slider_accel * direction).clamp(min, max);
        let new_cv = ControlValue::Float(new_val);
        self.control_values.insert(ctrl.id.clone(), new_cv.clone());
        Some(Action::UpdateControl(ctrl.id, new_cv))
    }

    fn toggle_bool(&mut self, ctrl_idx: usize) -> Option<Action> {
        let ctrl = self
            .effects
            .get(self.selected_index)?
            .controls
            .get(ctrl_idx)?
            .clone();
        let current = current_value(&self.control_values, &ctrl)
            .as_bool()
            .unwrap_or(false);
        let new_cv = ControlValue::Boolean(!current);
        self.control_values.insert(ctrl.id.clone(), new_cv.clone());
        Some(Action::UpdateControl(ctrl.id, new_cv))
    }

    fn cycle_dropdown(&mut self, ctrl_idx: usize, forward: bool) -> Option<Action> {
        let ctrl = self
            .effects
            .get(self.selected_index)?
            .controls
            .get(ctrl_idx)?
            .clone();
        if ctrl.labels.is_empty() {
            return None;
        }
        let current_text = match current_value(&self.control_values, &ctrl) {
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
        Some(Action::UpdateControl(ctrl.id, new_cv))
    }

    // ── Mouse helpers ─────────────────────────────────────────────────

    /// Map a visual row offset (within the list area) to an effect index.
    fn effect_index_at_visual_row(&self, target_row: usize) -> Option<usize> {
        let mut visual_row = 0;
        let mut current_category = String::new();

        for (flat_idx, effect) in self.effects.iter().enumerate() {
            if effect.category != current_category {
                if !current_category.is_empty() {
                    visual_row += 1; // blank separator
                }
                visual_row += 1; // category header
                current_category.clone_from(&effect.category);
            }

            if visual_row == target_row {
                return Some(flat_idx);
            }
            visual_row += 1;
        }
        None
    }

    fn select_effect_at_row(&mut self, row: u16) {
        let inner = self.list_inner_rect.get();
        let list_content_y = inner.y + 2; // after search bar + gap
        if row < list_content_y {
            return;
        }
        let visible_row = usize::from(row - list_content_y);
        let absolute_row = visible_row + self.scroll_offset.get();
        if let Some(idx) = self.effect_index_at_visual_row(absolute_row)
            && idx != self.selected_index
        {
            self.selected_index = idx;
            self.selected_preset = 0;
            self.sync_controls_to_selection();
        }
    }

    fn select_control_at_row(&mut self, row: u16) {
        let start_y = self.controls_content_y.get();
        if row < start_y {
            return;
        }
        let offset = usize::from(row - start_y);
        let ctrl_idx = offset / 2; // 2 rows per control (content + padding)
        let ctrl_count = self.selected_effect().map_or(0, |e| e.controls.len());
        if ctrl_idx < ctrl_count {
            self.selected_control = ctrl_idx;
        }
    }

    /// Set a slider value by mapping mouse column to normalized [0, 1].
    #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
    fn set_slider_by_mouse(&mut self, col: u16, row: u16) -> Option<Action> {
        let start_y = self.controls_content_y.get();
        if row < start_y {
            return None;
        }
        let offset = usize::from(row - start_y);
        let ctrl_idx = offset / 2;
        self.selected_control = ctrl_idx;

        let ctrl = self
            .effects
            .get(self.selected_index)?
            .controls
            .get(ctrl_idx)?
            .clone();
        if ctrl.control_type != "slider" {
            return None;
        }

        // The slider bar starts after: inner.x + 2 (selection bar pad) + label_w + 1
        let inner = self.controls_inner_rect.get();
        let label_w = self.controls_label_w.get();
        #[allow(clippy::cast_possible_truncation)]
        let bar_start = inner.x + 2 + label_w as u16 + 1;
        let bar_end = inner.x + inner.width.saturating_sub(1);
        let bar_width = bar_end.saturating_sub(bar_start);

        if bar_width == 0 || col < bar_start {
            return None;
        }

        let t = (f32::from(col - bar_start) / f32::from(bar_width)).clamp(0.0, 1.0);
        let min = ctrl.min.unwrap_or(0.0);
        let max = ctrl.max.unwrap_or(1.0);
        let new_val = min + t * (max - min);
        let new_cv = ControlValue::Float(new_val);
        self.control_values.insert(ctrl.id.clone(), new_cv.clone());
        Some(Action::UpdateControl(ctrl.id, new_cv))
    }

    // ── Color picker ─────────────────────────────────────────────────

    fn open_color_picker(&mut self) {
        let idx = self.selected_control;
        let Some(ctrl) = self
            .effects
            .get(self.selected_index)
            .and_then(|e| e.controls.get(idx).cloned())
        else {
            return;
        };
        if ctrl.control_type == "color" {
            let color_arr = match current_value(&self.control_values, &ctrl) {
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

    fn confirm_color_picker(&mut self) -> Option<Action> {
        let picker = self.color_picker.take()?;
        let ctrl = self
            .effects
            .get(self.selected_index)?
            .controls
            .get(picker.ctrl_idx)?
            .clone();
        let (r, g, b) = hsl_to_rgb(picker.hsl[0], picker.hsl[1], picker.hsl[2]);
        let alpha = match current_value(&self.control_values, &ctrl) {
            ControlValue::Color(c) => c[3],
            _ => 1.0,
        };
        let new_cv = ControlValue::Color([r, g, b, alpha]);
        self.control_values.insert(ctrl.id.clone(), new_cv.clone());
        Some(Action::UpdateControl(ctrl.id, new_cv))
    }

    fn handle_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        let picker = self.color_picker.as_mut()?;
        match key.code {
            KeyCode::Esc => {
                self.color_picker = None;
                None
            }
            KeyCode::Enter => self.confirm_color_picker(),
            KeyCode::Down => {
                picker.selected_channel = (picker.selected_channel + 1).min(2);
                None
            }
            KeyCode::Up => {
                picker.selected_channel = picker.selected_channel.saturating_sub(1);
                None
            }
            KeyCode::Left => {
                let step = match picker.selected_channel {
                    0 => -5.0,
                    _ => -0.02,
                };
                adjust_hsl_channel(&mut picker.hsl, picker.selected_channel, step);
                None
            }
            KeyCode::Right => {
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

    // ── Key handlers ─────────────────────────────────────────────────

    fn handle_search_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Esc => {
                self.search_active = false;
                self.search_query.clear();
                self.apply_filter();
                None
            }
            KeyCode::Enter => {
                self.search_active = false;
                None
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                self.apply_filter();
                None
            }
            KeyCode::Char(c) => {
                self.search_query.push(c);
                self.apply_filter();
                None
            }
            _ => None,
        }
    }

    fn handle_list_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Down => {
                if !self.effects.is_empty() {
                    let new = (self.selected_index + 1).min(self.effects.len().saturating_sub(1));
                    if new != self.selected_index {
                        self.selected_index = new;
                        self.selected_preset = 0;
                        self.sync_controls_to_selection();
                    }
                }
                None
            }
            KeyCode::Up => {
                let new = self.selected_index.saturating_sub(1);
                if new != self.selected_index {
                    self.selected_index = new;
                    self.selected_preset = 0;
                    self.sync_controls_to_selection();
                }
                None
            }
            KeyCode::Home => {
                self.selected_index = 0;
                self.selected_preset = 0;
                self.scroll_offset.set(0);
                self.sync_controls_to_selection();
                None
            }
            KeyCode::End => {
                if !self.effects.is_empty() {
                    self.selected_index = self.effects.len().saturating_sub(1);
                    self.selected_preset = 0;
                    self.sync_controls_to_selection();
                }
                None
            }
            KeyCode::PageDown => {
                if !self.effects.is_empty() {
                    self.selected_index =
                        (self.selected_index + 10).min(self.effects.len().saturating_sub(1));
                    self.selected_preset = 0;
                    self.sync_controls_to_selection();
                }
                None
            }
            KeyCode::PageUp => {
                self.selected_index = self.selected_index.saturating_sub(10);
                self.selected_preset = 0;
                self.sync_controls_to_selection();
                None
            }
            KeyCode::Char('/') => {
                self.search_active = true;
                None
            }
            KeyCode::Enter => self
                .selected_effect()
                .map(|e| Action::ApplyEffect(e.id.clone())),
            KeyCode::Char('f') => self
                .selected_effect()
                .map(|e| Action::ToggleFavorite(e.id.clone())),
            _ => None,
        }
    }

    fn handle_preview_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Down => {
                let count = self.selected_effect().map_or(0, |e| e.presets.len());
                if count > 0 {
                    self.selected_preset = (self.selected_preset + 1).min(count.saturating_sub(1));
                }
                None
            }
            KeyCode::Up => {
                self.selected_preset = self.selected_preset.saturating_sub(1);
                None
            }
            KeyCode::Enter => {
                if let Some(effect) = self.selected_effect() {
                    if let Some(preset) = effect.presets.get(self.selected_preset) {
                        Some(Action::ApplyEffectPreset(
                            effect.id.clone(),
                            preset.controls.clone(),
                        ))
                    } else {
                        Some(Action::ApplyEffect(effect.id.clone()))
                    }
                } else {
                    None
                }
            }
            KeyCode::Char('f') => self
                .selected_effect()
                .map(|e| Action::ToggleFavorite(e.id.clone())),
            KeyCode::Char('/') => {
                self.search_active = true;
                self.focus_pane = FocusPane::List;
                None
            }
            KeyCode::Home => {
                self.selected_preset = 0;
                None
            }
            KeyCode::End => {
                let count = self.selected_effect().map_or(0, |e| e.presets.len());
                if count > 0 {
                    self.selected_preset = count.saturating_sub(1);
                }
                None
            }
            _ => None,
        }
    }

    fn handle_controls_key(&mut self, key: KeyEvent) -> Option<Action> {
        let ctrl_count = self.selected_effect().map_or(0, |e| e.controls.len());

        if ctrl_count == 0 {
            return None;
        }

        match key.code {
            KeyCode::Down => {
                self.selected_control =
                    (self.selected_control + 1).min(ctrl_count.saturating_sub(1));
                None
            }
            KeyCode::Up => {
                self.selected_control = self.selected_control.saturating_sub(1);
                None
            }
            KeyCode::Left => {
                let idx = self.selected_control;
                let ctrl_type = self
                    .selected_effect()
                    .and_then(|e| e.controls.get(idx))
                    .map(|c| c.control_type.clone());
                match ctrl_type.as_deref() {
                    Some("slider") => self.adjust_slider(idx, -1.0),
                    Some("dropdown") => self.cycle_dropdown(idx, false),
                    _ => None,
                }
            }
            KeyCode::Right => {
                let idx = self.selected_control;
                let ctrl_type = self
                    .selected_effect()
                    .and_then(|e| e.controls.get(idx))
                    .map(|c| c.control_type.clone());
                match ctrl_type.as_deref() {
                    Some("slider") => self.adjust_slider(idx, 1.0),
                    Some("dropdown") => self.cycle_dropdown(idx, true),
                    _ => None,
                }
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                let idx = self.selected_control;
                let ctrl_type = self
                    .selected_effect()
                    .and_then(|e| e.controls.get(idx))
                    .map(|c| c.control_type.clone());
                match ctrl_type.as_deref() {
                    Some("toggle") => self.toggle_bool(idx),
                    Some("dropdown") => self.cycle_dropdown(idx, true),
                    Some("color") => {
                        self.open_color_picker();
                        None
                    }
                    _ => None,
                }
            }
            KeyCode::Home => {
                self.selected_control = 0;
                None
            }
            KeyCode::End => {
                self.selected_control = ctrl_count.saturating_sub(1);
                None
            }
            _ => None,
        }
    }

    // ── List pane rendering ──────────────────────────────────────────

    fn render_list_pane(&self, frame: &mut Frame, area: Rect) {
        let is_focused = self.focus_pane == FocusPane::List;
        let border_color = if is_focused { NEON_CYAN } else { BORDER_DIM };

        let block = Block::default()
            .title(Span::styled(
                " Effects ",
                Style::default()
                    .fg(ELECTRIC_PURPLE)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        self.list_inner_rect.set(inner);

        if inner.width < 4 || inner.height < 3 {
            return;
        }

        self.render_search_bar(frame, inner);
        self.render_effect_list(frame, inner);
    }

    fn render_search_bar(&self, frame: &mut Frame, inner: Rect) {
        let search_text = if self.search_active {
            format!("/ {}\u{2588}", self.search_query)
        } else if self.search_query.is_empty() {
            "/ Search...".to_string()
        } else {
            format!("/ {}", self.search_query)
        };

        let search_style = if self.search_active {
            Style::default().fg(NEON_CYAN)
        } else {
            Style::default().fg(DIM_GRAY)
        };

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(search_text, search_style))),
            Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), 1),
        );
    }

    fn render_effect_list(&self, frame: &mut Frame, inner: Rect) {
        let list_area = Rect::new(
            inner.x,
            inner.y + 2,
            inner.width,
            inner.height.saturating_sub(3),
        );

        if list_area.height == 0 {
            return;
        }

        let (items, selected_item_idx) = self.build_list_items(inner.width.saturating_sub(2));
        let visible = usize::from(list_area.height);
        let mut offset = self.scroll_offset.get();

        if let Some(sel) = selected_item_idx {
            if sel < offset {
                offset = sel;
            } else if sel >= offset + visible {
                offset = sel.saturating_sub(visible.saturating_sub(1));
            }
        }
        self.scroll_offset.set(offset);

        let visible_items: Vec<ListItem<'_>> =
            items.into_iter().skip(offset).take(visible).collect();

        frame.render_widget(List::new(visible_items), list_area);
    }

    fn build_list_items(&self, avail_width: u16) -> (Vec<ListItem<'_>>, Option<usize>) {
        let mut items: Vec<ListItem<'_>> = Vec::new();
        let mut current_category = String::new();
        let mut selected_item_idx = None;

        for (flat_idx, effect) in self.effects.iter().enumerate() {
            if effect.category != current_category {
                if !current_category.is_empty() {
                    items.push(ListItem::new(Line::from("")));
                }
                let cat_display = if effect.category.is_empty() {
                    "Uncategorized"
                } else {
                    &effect.category
                };
                items.push(ListItem::new(Line::from(Span::styled(
                    format!("\u{2500}\u{2500} {cat_display} \u{2500}\u{2500}"),
                    Style::default().fg(DIM_GRAY).add_modifier(Modifier::BOLD),
                ))));
                current_category.clone_from(&effect.category);
            }

            if flat_idx == self.selected_index {
                selected_item_idx = Some(items.len());
            }
            items.push(self.build_effect_item(flat_idx, effect, avail_width));
        }

        items.push(ListItem::new(Line::from("")));
        items.push(ListItem::new(Line::from(Span::styled(
            format!("{} effects", self.effects.len()),
            Style::default().fg(DIM_GRAY),
        ))));

        (items, selected_item_idx)
    }

    fn build_effect_item<'a>(
        &self,
        flat_idx: usize,
        effect: &'a EffectSummary,
        avail_width: u16,
    ) -> ListItem<'a> {
        let is_selected = flat_idx == self.selected_index;
        let is_fav = self.is_favorite(&effect.id);
        let pointer = if is_selected { "\u{25B8} " } else { "  " };
        let fav_marker = if is_fav { " \u{2605}" } else { "" };

        let source_badge = match effect.source.as_str() {
            "native" | "wgpu" => "\u{2726} native",
            "web" | "servo" => "\u{25C8} web",
            _ => "",
        };

        let name_style = if is_selected {
            Style::default().fg(NEON_CYAN).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(BASE_WHITE)
        };

        let mut spans = vec![
            Span::styled(pointer, name_style),
            Span::styled(&effect.name, name_style),
        ];

        if !fav_marker.is_empty() {
            spans.push(Span::styled(
                fav_marker,
                Style::default().fg(ELECTRIC_YELLOW),
            ));
        }

        if !source_badge.is_empty() {
            let name_len = pointer.len() + effect.name.len() + fav_marker.len();
            let avail = usize::from(avail_width);
            let pad = avail.saturating_sub(name_len + source_badge.len());
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(source_badge, Style::default().fg(DIM_GRAY)));
        }

        ListItem::new(Line::from(spans))
    }

    // ── Preview pane rendering ───────────────────────────────────────

    fn render_preview_pane(&self, frame: &mut Frame, area: Rect) {
        let is_focused = self.focus_pane == FocusPane::Preview;
        let border_color = if is_focused { NEON_CYAN } else { BORDER_DIM };

        // Title shows the effect name (or "Preview" when nothing selected)
        let title = self.selected_effect().map_or_else(
            || {
                vec![Span::styled(
                    " Preview ",
                    Style::default()
                        .fg(ELECTRIC_PURPLE)
                        .add_modifier(Modifier::BOLD),
                )]
            },
            |effect| {
                let fav = if self.is_favorite(&effect.id) {
                    " \u{2605}"
                } else {
                    ""
                };
                let author_part = if effect.author.is_empty() {
                    effect.source.clone()
                } else {
                    format!("{} \u{00B7} {}", effect.author, effect.source)
                };
                vec![
                    Span::styled(" ", Style::default()),
                    Span::styled(
                        &effect.name,
                        Style::default().fg(NEON_CYAN).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(fav, Style::default().fg(ELECTRIC_YELLOW)),
                    Span::styled(
                        format!(" \u{2500} {author_part} "),
                        Style::default().fg(DIM_GRAY),
                    ),
                ]
            },
        );

        let block = Block::default()
            .title(Line::from(title))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width < 4 || inner.height < 2 {
            return;
        }

        if self.selected_effect().is_none() {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "No effect selected",
                    Style::default().fg(DIM_GRAY),
                ))),
                Rect::new(inner.x + 1, inner.y + 1, inner.width.saturating_sub(2), 1),
            );
            return;
        }

        // Canvas fills the entire preview area
        self.render_canvas_preview(frame, inner);
    }

    fn render_canvas_preview(&self, frame: &mut Frame, area: Rect) {
        if let Some(ref cf) = self.canvas_frame {
            let fitted = aspect_fit(cf.width, cf.height, area);
            let canvas = HalfBlockCanvas::new(&cf.pixels, cf.width, cf.height);
            frame.render_widget(canvas, fitted);
        } else {
            let placeholder = Paragraph::new(Line::from(Span::styled(
                "\u{2584}".repeat(usize::from(area.width)),
                Style::default().fg(Color::Rgb(30, 30, 50)),
            )));
            frame.render_widget(placeholder, area);
        }
    }

    /// Render an inline preset selector at the given area (single row, right-aligned).
    ///
    /// Format: `◂ PresetName ▸` with ↑/↓ navigation hints when focused.
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    fn render_preset_indicator(&self, frame: &mut Frame, area: Rect) {
        let Some(effect) = self.selected_effect() else {
            return;
        };
        if effect.presets.is_empty() || area.width < 10 || area.height == 0 {
            return;
        }

        let preset = &effect.presets[self.selected_preset];
        let total = effect.presets.len();
        let is_focused = self.focus_pane == FocusPane::Preview;

        let mut spans: Vec<Span<'static>> = Vec::new();

        let nav_style = if is_focused {
            Style::default().fg(NEON_CYAN)
        } else {
            Style::default().fg(DIM_GRAY)
        };
        let name_style = if is_focused {
            Style::default().fg(NEON_CYAN).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(BASE_WHITE)
        };

        spans.push(Span::styled("Preset ", Style::default().fg(DIM_GRAY)));
        spans.push(Span::styled("\u{25C2} ", nav_style));
        spans.push(Span::styled(preset.name.clone(), name_style));
        spans.push(Span::styled(" \u{25B8}", nav_style));
        spans.push(Span::styled(
            format!(" {}/{} ", self.selected_preset + 1, total),
            Style::default().fg(DIM_GRAY),
        ));

        let content_width: u16 = spans.iter().map(|s| s.width() as u16).sum();
        let x = area.x + area.width.saturating_sub(content_width);
        let w = content_width.min(area.width);

        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(x, area.y, w, 1),
        );
    }

    // ── Controls pane rendering ──────────────────────────────────────

    #[allow(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::too_many_lines
    )]
    fn render_controls_pane(&self, frame: &mut Frame, area: Rect) {
        let is_focused = self.focus_pane == FocusPane::Controls;
        let border_color = if is_focused { NEON_CYAN } else { BORDER_DIM };

        let effect_name = self.selected_effect().map_or("Controls", |e| &e.name);

        let block = Block::default()
            .title(Line::from(vec![
                Span::styled(" ", Style::default().fg(ELECTRIC_PURPLE)),
                Span::styled(
                    effect_name,
                    Style::default()
                        .fg(ELECTRIC_PURPLE)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" \u{2500} Controls ", Style::default().fg(BORDER_DIM)),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        self.controls_inner_rect.set(inner);

        if inner.width < 10 || inner.height < 2 {
            return;
        }

        let Some(effect_ref) = self.selected_effect() else {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "No effect selected",
                    Style::default().fg(DIM_GRAY),
                )),
                Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), 1),
            );
            return;
        };
        let controls = &effect_ref.controls;

        if controls.is_empty() {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "No adjustable controls",
                    Style::default().fg(DIM_GRAY),
                )),
                Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), 1),
            );
            return;
        }

        // Description (1 line max)
        let mut content_y = inner.y;
        if !effect_ref.description.is_empty() {
            frame.render_widget(
                Paragraph::new(effect_ref.description.as_str())
                    .style(Style::default().fg(DIM_GRAY))
                    .wrap(Wrap { trim: true }),
                Rect::new(inner.x + 1, content_y, inner.width.saturating_sub(2), 1),
            );
            content_y += 2;
        }

        let label_w = controls
            .iter()
            .map(|c| c.name.chars().count())
            .max()
            .unwrap_or(0)
            .max(4);
        let ctrl_width = inner.width.saturating_sub(3);

        // Cache for mouse hit-testing
        self.controls_content_y.set(content_y);
        self.controls_label_w.set(label_w);

        for (i, ctrl) in controls.iter().enumerate() {
            if content_y >= inner.y + inner.height.saturating_sub(1) {
                break;
            }

            let is_selected = i == self.selected_control && is_focused;

            // Selection bar
            if is_selected {
                frame.render_widget(
                    Paragraph::new(Span::styled("\u{2503}", Style::default().fg(NEON_CYAN))),
                    Rect::new(inner.x, content_y, 1, 1),
                );
            }

            let ctrl_area = Rect::new(inner.x + 2, content_y, ctrl_width, 1);
            self.render_control(frame, ctrl, ctrl_area, is_selected, label_w);
            content_y += 2;
        }

        // Key hints
        let hint_y = inner.y + inner.height.saturating_sub(1);
        if hint_y > content_y {
            let hints = if is_focused {
                vec![
                    Span::styled("\u{2191}\u{2193}", Style::default().fg(ELECTRIC_YELLOW)),
                    Span::styled(" nav  ", Style::default().fg(DIM_GRAY)),
                    Span::styled("\u{2190}\u{2192}", Style::default().fg(ELECTRIC_YELLOW)),
                    Span::styled(" adjust  ", Style::default().fg(DIM_GRAY)),
                    Span::styled("Tab", Style::default().fg(ELECTRIC_YELLOW)),
                    Span::styled(" pane", Style::default().fg(DIM_GRAY)),
                ]
            } else {
                vec![
                    Span::styled("Tab", Style::default().fg(ELECTRIC_YELLOW)),
                    Span::styled(" to focus controls", Style::default().fg(DIM_GRAY)),
                ]
            };
            frame.render_widget(
                Paragraph::new(Line::from(hints)),
                Rect::new(inner.x + 1, hint_y, inner.width.saturating_sub(2), 1),
            );
        }
    }

    /// Render a single control row.
    #[allow(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::too_many_lines
    )]
    fn render_control(
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

        match ctrl.control_type.as_str() {
            "slider" => {
                let norm = normalized_value(&self.control_values, ctrl);
                let (grad_from, grad_to) = if is_selected {
                    ((128, 255, 234), (225, 53, 255))
                } else {
                    ((255, 106, 193), (225, 53, 255))
                };
                let padded = format!("{:<width$}", ctrl.name, width = label_w + 1);
                let slider = ParamSlider::new(&padded, norm)
                    .gradient_fill(grad_from, grad_to)
                    .accent_color(NEON_CYAN);
                frame.render_widget(slider, area);
            }
            "toggle" => {
                let on = current_value(&self.control_values, ctrl)
                    .as_bool()
                    .unwrap_or(false);
                let name_style = selected_style(is_selected);
                let (indicator, state_text, color) = if on {
                    ("\u{25C9}", "On", SUCCESS_GREEN)
                } else {
                    ("\u{25CB}", "Off", DIM_GRAY)
                };
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(
                            format!("{:<width$}", ctrl.name, width = label_w + 1),
                            name_style,
                        ),
                        Span::styled(
                            format!("{indicator} {state_text}"),
                            Style::default().fg(color),
                        ),
                    ])),
                    area,
                );
            }
            "dropdown" => {
                let current_text = match current_value(&self.control_values, ctrl) {
                    ControlValue::Text(s) => s,
                    _ => String::new(),
                };
                let name_style = selected_style(is_selected);
                let val_color = if is_selected {
                    NEON_CYAN
                } else {
                    ELECTRIC_YELLOW
                };
                let arrows = if is_selected { " \u{25C0}\u{25B6}" } else { "" };
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(
                            format!("{:<width$}", ctrl.name, width = label_w + 1),
                            name_style,
                        ),
                        Span::styled(
                            format!("\u{25B8} {current_text}"),
                            Style::default().fg(val_color),
                        ),
                        Span::styled(arrows, Style::default().fg(DIM_GRAY)),
                    ])),
                    area,
                );
            }
            "color" => {
                let name_style = selected_style(is_selected);
                let color_arr = match current_value(&self.control_values, ctrl) {
                    ControlValue::Color(c) => c,
                    _ => [0.5, 0.5, 0.5, 1.0],
                };
                let r = float_to_u8(color_arr[0]);
                let g = float_to_u8(color_arr[1]);
                let b = float_to_u8(color_arr[2]);
                let swatch = Color::Rgb(r, g, b);
                let mut spans = vec![
                    Span::styled(
                        format!("{:<width$}", ctrl.name, width = label_w + 1),
                        name_style,
                    ),
                    Span::styled(
                        "\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}",
                        Style::default().fg(swatch),
                    ),
                    Span::styled(
                        format!("  #{r:02x}{g:02x}{b:02x}"),
                        Style::default().fg(BASE_WHITE).add_modifier(Modifier::BOLD),
                    ),
                ];
                if is_selected {
                    spans.push(Span::styled(
                        "  \u{25C8}Enter",
                        Style::default().fg(ELECTRIC_PURPLE),
                    ));
                }
                frame.render_widget(Paragraph::new(Line::from(spans)), area);
            }
            _ => {
                let ctrl_type = &ctrl.control_type;
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(
                            format!("{:<width$}", ctrl.name, width = label_w + 1),
                            Style::default().fg(BASE_WHITE),
                        ),
                        Span::styled(format!("({ctrl_type})"), Style::default().fg(DIM_GRAY)),
                    ])),
                    area,
                );
            }
        }
    }

    /// Render color picker popup overlay.
    fn render_color_picker(&self, frame: &mut Frame, area: Rect) {
        if let Some(ref picker) = self.color_picker {
            let ctrl_name = self
                .selected_effect()
                .and_then(|e| e.controls.get(picker.ctrl_idx))
                .map_or("Color", |c| &c.name);

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

// ── Free helpers ─────────────────────────────────────────────────────

/// Get the current value for a control, falling back to its default.
fn current_value(values: &HashMap<String, ControlValue>, ctrl: &ControlDefinition) -> ControlValue {
    values
        .get(&ctrl.id)
        .cloned()
        .unwrap_or_else(|| ctrl.default_value.clone())
}

/// Normalize a control's current value to `[0, 1]`.
fn normalized_value(values: &HashMap<String, ControlValue>, ctrl: &ControlDefinition) -> f32 {
    normalize(
        current_value(values, ctrl).as_f32().unwrap_or(0.0),
        ctrl.min.unwrap_or(0.0),
        ctrl.max.unwrap_or(1.0),
    )
}

fn normalize(raw: f32, min: f32, max: f32) -> f32 {
    if (max - min).abs() < f32::EPSILON {
        0.0
    } else {
        ((raw - min) / (max - min)).clamp(0.0, 1.0)
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
fn float_to_u8(value: f32) -> u8 {
    value.mul_add(255.0, 0.5).clamp(0.0, 255.0) as u8
}

fn selected_style(is_selected: bool) -> Style {
    if is_selected {
        Style::default().fg(NEON_CYAN).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(BASE_WHITE)
    }
}

fn adjust_hsl_channel(hsl: &mut [f32; 3], channel: usize, delta: f32) {
    match channel {
        0 => hsl[0] = (hsl[0] + delta).rem_euclid(360.0),
        1 => hsl[1] = (hsl[1] + delta).clamp(0.0, 1.0),
        _ => hsl[2] = (hsl[2] + delta).clamp(0.0, 1.0),
    }
}

/// Compute the largest sub-rect of `area` that preserves `src_w:src_h` aspect ratio.
/// Accounts for half-block rendering (2 vertical pixels per terminal row).
#[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
fn aspect_fit(src_w: u16, src_h: u16, area: Rect) -> Rect {
    if src_w == 0 || src_h == 0 || area.width == 0 || area.height == 0 {
        return area;
    }
    let sw = u32::from(src_w);
    let sh = u32::from(src_h);
    let tw = u32::from(area.width);
    let th = u32::from(area.height);

    // Each terminal row = 2 pixel rows (half-block)
    let fit_h_pixels = sh * tw / sw;
    let fit_h_rows = fit_h_pixels.div_ceil(2);

    let (rw, rh) = if fit_h_rows <= th {
        (tw as u16, fit_h_rows as u16)
    } else {
        let th2 = th * 2;
        let fit_w = sw * th2 / sh;
        (fit_w.min(tw) as u16, th as u16)
    };

    let x = area.x + (area.width.saturating_sub(rw)) / 2;
    let y = area.y + (area.height.saturating_sub(rh)) / 2;
    Rect::new(x, y, rw, rh)
}

fn rect_contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

// ── Component impl ──────────────────────────────────────────────────

impl Component for EffectBrowserView {
    fn init(&mut self, action_tx: UnboundedSender<Action>) -> Result<()> {
        self.action_tx = Some(action_tx);
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        // Color picker intercepts everything
        if self.color_picker.is_some() {
            return Ok(self.handle_picker_key(key));
        }

        if self.search_active {
            return Ok(self.handle_search_key(key));
        }

        // Esc clears search or returns focus to list
        if key.code == KeyCode::Esc {
            if !self.search_query.is_empty() {
                self.search_query.clear();
                self.apply_filter();
                return Ok(None);
            }
            if self.focus_pane != FocusPane::List {
                self.focus_pane = FocusPane::List;
                return Ok(None);
            }
        }

        // Tab / Shift+Tab cycles focus between panes
        match key.code {
            KeyCode::Tab => {
                self.focus_pane = match self.focus_pane {
                    FocusPane::List => FocusPane::Preview,
                    FocusPane::Preview => FocusPane::Controls,
                    FocusPane::Controls => FocusPane::List,
                };
                return Ok(None);
            }
            KeyCode::BackTab => {
                self.focus_pane = match self.focus_pane {
                    FocusPane::List => FocusPane::Controls,
                    FocusPane::Preview => FocusPane::List,
                    FocusPane::Controls => FocusPane::Preview,
                };
                return Ok(None);
            }
            _ => {}
        }

        Ok(match self.focus_pane {
            FocusPane::List => self.handle_list_key(key),
            FocusPane::Preview => self.handle_preview_key(key),
            FocusPane::Controls => self.handle_controls_key(key),
        })
    }

    fn handle_mouse_event(&mut self, mouse: MouseEvent) -> Result<Option<Action>> {
        // Resizable splits take priority — consume drag events on dividers
        if self.h_split.handle_mouse(&mouse) || self.v_split.handle_mouse(&mouse) {
            return Ok(None);
        }

        let col = mouse.column;
        let row = mouse.row;
        let list_r = self.list_rect.get();
        let preview_r = self.preview_rect.get();
        let controls_r = self.controls_rect.get();

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if rect_contains(list_r, col, row) {
                    self.focus_pane = FocusPane::List;
                    self.select_effect_at_row(row);
                } else if rect_contains(preview_r, col, row) {
                    self.focus_pane = FocusPane::Preview;
                } else if rect_contains(controls_r, col, row) {
                    self.focus_pane = FocusPane::Controls;
                    self.select_control_at_row(row);
                    // Click-to-set slider value
                    if let Some(action) = self.set_slider_by_mouse(col, row) {
                        return Ok(Some(action));
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if rect_contains(controls_r, col, row) {
                    // Drag to adjust slider in real-time
                    if let Some(action) = self.set_slider_by_mouse(col, row) {
                        return Ok(Some(action));
                    }
                }
            }
            MouseEventKind::ScrollDown => {
                if rect_contains(list_r, col, row) {
                    if !self.effects.is_empty() {
                        let new =
                            (self.selected_index + 1).min(self.effects.len().saturating_sub(1));
                        if new != self.selected_index {
                            self.selected_index = new;
                            self.selected_preset = 0;
                            self.sync_controls_to_selection();
                        }
                    }
                } else if rect_contains(controls_r, col, row) {
                    let ctrl_count = self.selected_effect().map_or(0, |e| e.controls.len());
                    if ctrl_count > 0 {
                        self.selected_control =
                            (self.selected_control + 1).min(ctrl_count.saturating_sub(1));
                    }
                } else if rect_contains(preview_r, col, row) {
                    let count = self.selected_effect().map_or(0, |e| e.presets.len());
                    if count > 0 {
                        self.selected_preset =
                            (self.selected_preset + 1).min(count.saturating_sub(1));
                    }
                }
            }
            MouseEventKind::ScrollUp => {
                if rect_contains(list_r, col, row) {
                    let new = self.selected_index.saturating_sub(1);
                    if new != self.selected_index {
                        self.selected_index = new;
                        self.selected_preset = 0;
                        self.sync_controls_to_selection();
                    }
                } else if rect_contains(controls_r, col, row) {
                    self.selected_control = self.selected_control.saturating_sub(1);
                } else if rect_contains(preview_r, col, row) {
                    self.selected_preset = self.selected_preset.saturating_sub(1);
                }
            }
            _ => {}
        }

        Ok(None)
    }

    fn update(&mut self, action: &Action) -> Result<Option<Action>> {
        match action {
            Action::EffectsUpdated(effects) => {
                self.all_effects.clone_from(effects);
                self.apply_filter();
                self.sync_controls_to_selection();
            }
            Action::FavoritesUpdated(favs) => {
                self.favorites.clone_from(favs);
            }
            Action::CanvasFrameReceived(cf) => {
                self.canvas_frame = Some(Arc::clone(cf));
            }
            _ => {}
        }
        Ok(None)
    }

    fn render(&self, frame: &mut Frame, area: Rect) {
        if area.width < 4 || area.height < 4 {
            return;
        }

        // Resizable 3-pane: list | preview / controls
        let [list_area, right_area] = self.h_split.layout(area);
        let [preview_area, controls_area] = self.v_split.layout(right_area);

        // Cache rects for mouse hit-testing
        self.list_rect.set(list_area);
        self.preview_rect.set(preview_area);
        self.controls_rect.set(controls_area);

        self.render_list_pane(frame, list_area);
        self.render_preview_pane(frame, preview_area);
        self.render_controls_pane(frame, controls_area);

        // Split divider overlays (highlight on hover/drag)
        self.h_split.render_divider(frame);
        self.v_split.render_divider(frame);

        // Preset indicator overlays the bottom-right border of the controls pane
        if controls_area.height > 0 {
            let bottom_row = Rect::new(
                controls_area.x + 1,
                controls_area.y + controls_area.height.saturating_sub(1),
                controls_area.width.saturating_sub(2),
                1,
            );
            self.render_preset_indicator(frame, bottom_row);
        }

        // Color picker overlay on top
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
        "effect_browser"
    }
}
