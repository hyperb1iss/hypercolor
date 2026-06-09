//! Scene picker modal — list saved scenes, activate one, or return to Default.
//!
//! Opened with `c`. Arrow keys / j/k navigate, Enter activates, Esc closes.
//! The first row is always the ephemeral "Default" scene (mapped to
//! `POST /scenes/deactivate`), mirroring the web UI's "Return to Default".

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use hypercolor_types::scene::{SceneKind, SceneMutationMode};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::state::AppState;
use crate::theme;

/// What the app should do after the picker handled an input event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScenePickerAction {
    /// Nothing — picker consumed the event internally.
    None,
    /// Close the picker without changes.
    Close,
    /// Activate the scene with this ID.
    Activate(String),
    /// Return to the ephemeral default scene.
    Deactivate,
}

#[derive(Debug, Clone)]
struct SceneEntry {
    /// `None` = the Default row.
    id: Option<String>,
    name: String,
    locked: bool,
    active: bool,
}

/// Modal scene picker. `None` when closed (held by `App`).
pub struct ScenePicker {
    entries: Vec<SceneEntry>,
    selected: usize,
    /// Modal rect from the last render, for mouse hit-testing.
    last_area: Rect,
}

impl ScenePicker {
    /// Open the picker with the active scene pre-selected.
    #[must_use]
    pub fn open(state: &AppState) -> Self {
        let mut picker = Self {
            entries: Vec::new(),
            selected: 0,
            last_area: Rect::default(),
        };
        picker.sync(state);
        picker.selected = picker
            .entries
            .iter()
            .position(|entry| entry.active)
            .unwrap_or(0);
        picker
    }

    /// Rebuild entries from app state (scene list or active scene changed).
    pub fn sync(&mut self, state: &AppState) {
        let active = state.active_scene.as_deref();
        let default_active = active.is_none_or(|scene| scene.kind == SceneKind::Ephemeral);

        let mut entries = vec![SceneEntry {
            id: None,
            name: "Default".to_string(),
            locked: false,
            active: default_active,
        }];
        entries.extend(state.scenes.iter().map(|scene| SceneEntry {
            id: Some(scene.id.clone()),
            name: scene.name.clone(),
            locked: scene.mutation_mode == SceneMutationMode::Snapshot,
            active: active.is_some_and(|current| current.id == scene.id),
        }));

        self.entries = entries;
        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
    }

    /// Handle a key event while the picker is open.
    pub fn handle_key(&mut self, key: KeyEvent) -> ScenePickerAction {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'c') => ScenePickerAction::Close,
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(self.entries.len().saturating_sub(1));
                ScenePickerAction::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                ScenePickerAction::None
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.selected = 0;
                ScenePickerAction::None
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.selected = self.entries.len().saturating_sub(1);
                ScenePickerAction::None
            }
            KeyCode::Enter => self.activate_selected(),
            _ => ScenePickerAction::None,
        }
    }

    /// Handle a mouse event while the picker is open.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> ScenePickerAction {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(index) = self.entry_at(mouse.column, mouse.row) else {
                    return ScenePickerAction::Close;
                };
                self.selected = index;
                self.activate_selected()
            }
            MouseEventKind::Down(MouseButton::Right) => ScenePickerAction::Close,
            MouseEventKind::ScrollDown => {
                self.selected = (self.selected + 1).min(self.entries.len().saturating_sub(1));
                ScenePickerAction::None
            }
            MouseEventKind::ScrollUp => {
                self.selected = self.selected.saturating_sub(1);
                ScenePickerAction::None
            }
            _ => ScenePickerAction::None,
        }
    }

    fn activate_selected(&self) -> ScenePickerAction {
        match self.entries.get(self.selected) {
            Some(entry) if entry.active => ScenePickerAction::Close,
            Some(SceneEntry { id: Some(id), .. }) => ScenePickerAction::Activate(id.clone()),
            Some(SceneEntry { id: None, .. }) => ScenePickerAction::Deactivate,
            None => ScenePickerAction::Close,
        }
    }

    fn entry_at(&self, col: u16, row: u16) -> Option<usize> {
        let area = self.last_area;
        if col < area.x + 1
            || col >= area.x + area.width.saturating_sub(1)
            || row < area.y + 1
            || row >= area.y + area.height.saturating_sub(1)
        {
            return None;
        }
        let index = usize::from(row - area.y - 1);
        (index < self.entries.len()).then_some(index)
    }

    /// Render the picker as a centered modal.
    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let width = 44u16.min(area.width.saturating_sub(4));
        let height = ((self.entries.len() as u16) + 2).min(area.height.saturating_sub(4));
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        let modal_area = Rect::new(x, y, width, height);
        self.last_area = modal_area;

        frame.render_widget(Clear, modal_area);

        let block = Block::default()
            .title(Span::styled(
                " Scenes ",
                Style::default()
                    .fg(theme::accent_primary())
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::accent_primary()))
            .style(Style::default().bg(theme::bg_panel()));
        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        let visible = usize::from(inner.height);
        let offset = self
            .selected
            .saturating_sub(visible.saturating_sub(1))
            .min(self.entries.len().saturating_sub(visible.max(1)));

        for (row, (index, entry)) in self
            .entries
            .iter()
            .enumerate()
            .skip(offset)
            .take(visible)
            .enumerate()
        {
            let is_selected = index == self.selected;
            let pointer = if is_selected { "\u{25B8} " } else { "  " };
            let name_style = if is_selected {
                Style::default()
                    .fg(theme::accent_secondary())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::text_primary())
            };

            let mut spans = vec![
                Span::styled(pointer, name_style),
                Span::styled(entry.name.clone(), name_style),
            ];
            if entry.locked {
                spans.push(Span::styled(
                    " [snap]",
                    Style::default().fg(theme::warning()),
                ));
            }
            if entry.active {
                spans.push(Span::styled(
                    " \u{25CF} active",
                    Style::default().fg(theme::success()),
                ));
            }

            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(
                    inner.x + 1,
                    inner.y + row as u16,
                    inner.width.saturating_sub(2),
                    1,
                ),
            );
        }
    }
}
