//! Effect browser view — browse, search, preview, and apply effects.

use std::sync::Arc;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use tokio::sync::mpsc::UnboundedSender;

use crate::action::Action;
use crate::component::Component;
use crate::state::{CanvasFrame, ControlValue, EffectSummary};
use crate::widgets::{HalfBlockCanvas, ParamSlider};

// ── SilkCircuit Neon palette ───────────────────────────────────────────

const NEON_CYAN: Color = Color::Rgb(128, 255, 234);
const ELECTRIC_PURPLE: Color = Color::Rgb(225, 53, 255);
const CORAL: Color = Color::Rgb(255, 106, 193);
const ELECTRIC_YELLOW: Color = Color::Rgb(241, 250, 140);
const SUCCESS_GREEN: Color = Color::Rgb(80, 250, 123);
const BASE_WHITE: Color = Color::Rgb(248, 248, 242);
const DIM_GRAY: Color = Color::Rgb(98, 114, 164);
const BORDER_DIM: Color = Color::Rgb(90, 21, 102);

/// Focus panel within the effect browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusPane {
    List,
    Preview,
}

// ── Effect Browser View ────────────────────────────────────────────────

/// Two-pane effect browser with search, category grouping, and canvas preview.
pub struct EffectBrowserView {
    focused: bool,
    action_tx: Option<UnboundedSender<Action>>,
    focus_pane: FocusPane,

    // Data
    all_effects: Vec<EffectSummary>,
    effects: Vec<EffectSummary>,
    favorites: Vec<String>,
    selected_index: usize,

    // Search
    search_active: bool,
    search_query: String,

    // Canvas preview
    canvas_frame: Option<Arc<CanvasFrame>>,
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
            search_active: false,
            search_query: String::new(),
            canvas_frame: None,
        }
    }

    /// Apply the current search query to filter effects.
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
    }

    /// Clamp the selected index to valid bounds.
    fn clamp_selection(&mut self) {
        if self.effects.is_empty() {
            self.selected_index = 0;
        } else {
            self.selected_index = self
                .selected_index
                .min(self.effects.len().saturating_sub(1));
        }
    }

    /// Check if an effect is in the favorites list.
    fn is_favorite(&self, id: &str) -> bool {
        self.favorites.iter().any(|f| f == id)
    }

    /// Get the currently selected effect, if any.
    fn selected_effect(&self) -> Option<&EffectSummary> {
        self.effects.get(self.selected_index)
    }

    /// Handle key events when the search bar is active.
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

    /// Handle key events when navigating the effect list.
    fn handle_list_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.effects.is_empty() {
                    self.selected_index =
                        (self.selected_index + 1).min(self.effects.len().saturating_sub(1));
                }
                None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected_index = self.selected_index.saturating_sub(1);
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
            KeyCode::Tab => {
                self.focus_pane = match self.focus_pane {
                    FocusPane::List => FocusPane::Preview,
                    FocusPane::Preview => FocusPane::List,
                };
                None
            }
            KeyCode::Char('g') => {
                self.selected_index = 0;
                None
            }
            KeyCode::Char('G') => {
                if !self.effects.is_empty() {
                    self.selected_index = self.effects.len().saturating_sub(1);
                }
                None
            }
            _ => None,
        }
    }

    // ── Panel renderers ─────────────────────────────────────────────

    /// Render the left pane: effect list with search and category grouping.
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

        if inner.width < 4 || inner.height < 3 {
            return;
        }

        self.render_search_bar(frame, inner);
        self.render_effect_list(frame, inner);
    }

    /// Render the search bar at the top of the list pane.
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

    /// Render the scrollable list of effects grouped by category.
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

        let items = self.build_list_items(inner.width.saturating_sub(2));

        frame.render_widget(List::new(items), list_area);
    }

    /// Build `ListItem`s for the effect list with category headers.
    fn build_list_items(&self, avail_width: u16) -> Vec<ListItem<'_>> {
        let mut items: Vec<ListItem<'_>> = Vec::new();
        let mut current_category = String::new();

        for (flat_idx, effect) in self.effects.iter().enumerate() {
            // Category header
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

            items.push(self.build_effect_item(flat_idx, effect, avail_width));
        }

        // Footer
        items.push(ListItem::new(Line::from("")));
        items.push(ListItem::new(Line::from(Span::styled(
            format!("{} effects", self.effects.len()),
            Style::default().fg(DIM_GRAY),
        ))));

        items
    }

    /// Build a single effect list item.
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

    /// Render the right pane: canvas preview, metadata, control summary.
    fn render_preview_pane(&self, frame: &mut Frame, area: Rect) {
        let is_focused = self.focus_pane == FocusPane::Preview;
        let border_color = if is_focused { NEON_CYAN } else { BORDER_DIM };

        let block = Block::default()
            .title(Span::styled(
                " Preview ",
                Style::default()
                    .fg(ELECTRIC_PURPLE)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width < 4 || inner.height < 4 {
            return;
        }

        let Some(effect) = self.selected_effect() else {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "No effect selected",
                    Style::default().fg(DIM_GRAY),
                ))),
                Rect::new(inner.x + 1, inner.y + 1, inner.width.saturating_sub(2), 1),
            );
            return;
        };

        // Split: canvas top, metadata bottom
        let canvas_height = inner.height.clamp(2, 8);
        let sections =
            Layout::vertical([Constraint::Length(canvas_height), Constraint::Fill(1)]).split(inner);

        self.render_canvas_preview(frame, sections[0]);
        self.render_metadata(frame, sections[1], effect);
    }

    /// Render the canvas preview area.
    fn render_canvas_preview(&self, frame: &mut Frame, area: Rect) {
        if let Some(ref cf) = self.canvas_frame {
            let canvas = HalfBlockCanvas::new(&cf.pixels, cf.width, cf.height);
            frame.render_widget(canvas, area);
        } else {
            let placeholder = Paragraph::new(Line::from(Span::styled(
                "\u{2584}".repeat(usize::from(area.width)),
                Style::default().fg(Color::Rgb(30, 30, 50)),
            )));
            frame.render_widget(placeholder, area);
        }
    }

    /// Render effect metadata and control summary.
    fn render_metadata(&self, frame: &mut Frame, area: Rect, effect: &EffectSummary) {
        if area.height < 2 {
            return;
        }

        let x_pad = area.x + 1;
        let w_pad = area.width.saturating_sub(2);
        let mut y = area.y;

        y = Self::render_meta_header(frame, x_pad, y, w_pad, area, effect);
        y = self.render_meta_info(frame, x_pad, y, w_pad, area, effect);
        Self::render_meta_controls(frame, x_pad, y, w_pad, area, effect);
    }

    /// Render effect name and author in the metadata section. Returns next y.
    fn render_meta_header(
        frame: &mut Frame,
        x: u16,
        mut y: u16,
        w: u16,
        area: Rect,
        effect: &EffectSummary,
    ) -> u16 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                &effect.name,
                Style::default().fg(BASE_WHITE).add_modifier(Modifier::BOLD),
            ))),
            Rect::new(x, y, w, 1),
        );
        y += 1;

        if y < area.y + area.height {
            let author_text = if effect.author.is_empty() {
                effect.source.clone()
            } else {
                format!("by {} \u{00B7} {}", effect.author, effect.source)
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    author_text,
                    Style::default().fg(DIM_GRAY),
                ))),
                Rect::new(x, y, w, 1),
            );
            y += 1;
        }

        y + 1 // blank line
    }

    /// Render description, audio reactive, params, favorite. Returns next y.
    #[allow(clippy::too_many_arguments)]
    fn render_meta_info(
        &self,
        frame: &mut Frame,
        x: u16,
        mut y: u16,
        w: u16,
        area: Rect,
        effect: &EffectSummary,
    ) -> u16 {
        let max_y = area.y + area.height;

        // Description
        if y < max_y && !effect.description.is_empty() {
            let desc_height = (max_y - y).min(3);
            frame.render_widget(
                Paragraph::new(effect.description.as_str())
                    .style(Style::default().fg(BASE_WHITE))
                    .wrap(Wrap { trim: true }),
                Rect::new(x, y, w, desc_height),
            );
            y += desc_height + 1;
        }

        // Audio reactive
        if y < max_y {
            let (audio_text, audio_color) = if effect.audio_reactive {
                ("yes", SUCCESS_GREEN)
            } else {
                ("no", DIM_GRAY)
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("Audio reactive: ", Style::default().fg(DIM_GRAY)),
                    Span::styled(audio_text, Style::default().fg(audio_color)),
                ])),
                Rect::new(x, y, w, 1),
            );
            y += 1;
        }

        // Parameters count
        if y < max_y {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("Parameters: ", Style::default().fg(DIM_GRAY)),
                    Span::styled(
                        effect.controls.len().to_string(),
                        Style::default().fg(CORAL),
                    ),
                ])),
                Rect::new(x, y, w, 1),
            );
            y += 2;
        }

        // Favorite status
        if y < max_y {
            let fav = self.is_favorite(&effect.id);
            let (fav_text, fav_color) = if fav {
                ("\u{2605} Favorite (f to remove)", ELECTRIC_YELLOW)
            } else {
                ("\u{2606} Press f to favorite", DIM_GRAY)
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    fav_text,
                    Style::default().fg(fav_color),
                ))),
                Rect::new(x, y, w, 1),
            );
            y += 2;
        }

        y
    }

    /// Render inline control summaries.
    fn render_meta_controls(
        frame: &mut Frame,
        x: u16,
        mut y: u16,
        w: u16,
        area: Rect,
        effect: &EffectSummary,
    ) {
        let max_y = area.y + area.height;

        if y >= max_y || effect.controls.is_empty() {
            return;
        }

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "\u{2500}\u{2500}\u{2500} Controls \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
                Style::default().fg(DIM_GRAY),
            ))),
            Rect::new(x, y, w, 1),
        );
        y += 1;

        for ctrl in effect.controls.iter().take(4) {
            if y >= max_y {
                break;
            }

            match ctrl.control_type.as_str() {
                "slider" => {
                    let norm = normalize_control(
                        ctrl.default_value.as_f32().unwrap_or(0.0),
                        ctrl.min.unwrap_or(0.0),
                        ctrl.max.unwrap_or(1.0),
                    );
                    let slider = ParamSlider::new(&ctrl.name, norm).accent_color(NEON_CYAN);
                    frame.render_widget(slider, Rect::new(x, y, w, 1));
                }
                "toggle" => {
                    let on = ctrl.default_value.as_bool().unwrap_or(false);
                    let toggle_text = if on { "[\u{25CF}] On" } else { "[ ] Off" };
                    let toggle_color = if on { SUCCESS_GREEN } else { DIM_GRAY };
                    frame.render_widget(
                        Paragraph::new(Line::from(vec![
                            Span::styled(
                                format!("{:<12}", ctrl.name),
                                Style::default().fg(BASE_WHITE).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(toggle_text, Style::default().fg(toggle_color)),
                        ])),
                        Rect::new(x, y, w, 1),
                    );
                }
                "dropdown" => {
                    let text = match &ctrl.default_value {
                        ControlValue::Text(s) => s.as_str(),
                        _ => "",
                    };
                    frame.render_widget(
                        Paragraph::new(Line::from(vec![
                            Span::styled(
                                format!("{:<12}", ctrl.name),
                                Style::default().fg(BASE_WHITE).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("[\u{25B8} {text}]"),
                                Style::default().fg(NEON_CYAN),
                            ),
                        ])),
                        Rect::new(x, y, w, 1),
                    );
                }
                _ => {
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(
                            format!("{}: ...", ctrl.name),
                            Style::default().fg(DIM_GRAY),
                        ))),
                        Rect::new(x, y, w, 1),
                    );
                }
            }
            y += 1;
        }
    }
}

/// Normalize a raw value to `[0, 1]` given min/max bounds.
fn normalize_control(raw: f32, min: f32, max: f32) -> f32 {
    if (max - min).abs() < f32::EPSILON {
        0.0
    } else {
        ((raw - min) / (max - min)).clamp(0.0, 1.0)
    }
}

impl Component for EffectBrowserView {
    fn init(&mut self, action_tx: UnboundedSender<Action>) -> Result<()> {
        self.action_tx = Some(action_tx);
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        if self.search_active {
            return Ok(self.handle_search_key(key));
        }

        // Esc clears search if query present
        if key.code == KeyCode::Esc && !self.search_query.is_empty() {
            self.search_query.clear();
            self.apply_filter();
            return Ok(None);
        }

        Ok(self.handle_list_key(key))
    }

    fn update(&mut self, action: &Action) -> Result<Option<Action>> {
        match action {
            Action::EffectsUpdated(effects) => {
                self.all_effects.clone_from(effects);
                self.apply_filter();
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

        let panes = Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(area);

        self.render_list_pane(frame, panes[0]);
        self.render_preview_pane(frame, panes[1]);
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
