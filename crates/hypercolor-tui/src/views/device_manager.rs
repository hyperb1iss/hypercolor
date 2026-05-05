//! Device manager view for generic dynamic control surfaces.

use std::cell::Cell as StdCell;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use hypercolor_types::controls::{
    ActionConfirmationLevel, ControlAccess, ControlActionDescriptor, ControlAvailabilityState,
    ControlFieldDescriptor, ControlGroupDescriptor, ControlSurfaceDocument, ControlSurfaceScope,
    ControlValue, ControlValueMap, ControlValueType,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};
use tokio::sync::mpsc::UnboundedSender;

use crate::action::Action;
use crate::component::Component;
use crate::state::DeviceSummary;

const NEON_CYAN: Color = Color::Rgb(128, 255, 234);
const ELECTRIC_PURPLE: Color = Color::Rgb(225, 53, 255);
const CORAL: Color = Color::Rgb(255, 106, 193);
const ELECTRIC_YELLOW: Color = Color::Rgb(241, 250, 140);
const BASE_WHITE: Color = Color::Rgb(248, 248, 242);
const DIM_GRAY: Color = Color::Rgb(98, 114, 164);
const BORDER_DIM: Color = Color::Rgb(90, 21, 102);

pub struct DeviceManagerView {
    focused: bool,
    action_tx: Option<UnboundedSender<Action>>,
    devices: Vec<DeviceSummary>,
    selected_device: usize,
    selected_control: usize,
    loaded_device_id: Option<String>,
    surfaces: Vec<ControlSurfaceDocument>,
    loading_device_id: Option<String>,
    error: Option<String>,
    pending_confirmation: Option<ControlKey>,
    confirmation_notice: Option<String>,
    devices_rect: StdCell<Rect>,
    devices_scroll: StdCell<usize>,
    controls_rect: StdCell<Rect>,
    controls_scroll: StdCell<usize>,
}

impl Default for DeviceManagerView {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceManagerView {
    #[must_use]
    pub fn new() -> Self {
        Self {
            focused: false,
            action_tx: None,
            devices: Vec::new(),
            selected_device: 0,
            selected_control: 0,
            loaded_device_id: None,
            surfaces: Vec::new(),
            loading_device_id: None,
            error: None,
            pending_confirmation: None,
            confirmation_notice: None,
            devices_rect: StdCell::new(Rect::default()),
            devices_scroll: StdCell::new(0),
            controls_rect: StdCell::new(Rect::default()),
            controls_scroll: StdCell::new(0),
        }
    }

    fn selected_device(&self) -> Option<&DeviceSummary> {
        self.devices.get(self.selected_device)
    }

    fn request_selected_controls(&mut self) -> Option<Action> {
        let device_id = self.selected_device()?.id.clone();
        self.loading_device_id = Some(device_id.clone());
        self.error = None;
        self.clear_pending_confirmation();
        Some(Action::LoadDeviceControls(device_id))
    }

    fn clear_pending_confirmation(&mut self) {
        self.pending_confirmation = None;
        self.confirmation_notice = None;
    }

    fn render_devices(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(Span::styled(
                " Devices ",
                Style::default()
                    .fg(ELECTRIC_PURPLE)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BORDER_DIM));

        let visible_rows = area.height.saturating_sub(3);
        let offset = self.visible_device_offset(usize::from(visible_rows));
        self.devices_scroll.set(offset);

        let rows = self
            .devices
            .iter()
            .enumerate()
            .skip(offset)
            .take(usize::from(visible_rows))
            .map(|(index, device)| {
                let selected = index == self.selected_device;
                let marker = if selected { "\u{25B8}" } else { " " };
                let style = if selected {
                    Style::default().fg(NEON_CYAN).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(BASE_WHITE)
                };
                Row::new(vec![
                    Cell::from(marker).style(style),
                    Cell::from(device.name.clone()).style(style),
                    Cell::from(device.state.clone()).style(Style::default().fg(DIM_GRAY)),
                    Cell::from(device.led_count.to_string()).style(Style::default().fg(CORAL)),
                ])
            });

        let table = Table::new(
            rows,
            [
                Constraint::Length(1),
                Constraint::Min(12),
                Constraint::Length(12),
                Constraint::Length(6),
            ],
        )
        .header(
            Row::new(["", "Name", "State", "LEDs"])
                .style(Style::default().fg(DIM_GRAY).add_modifier(Modifier::BOLD)),
        )
        .block(block);
        frame.render_widget(table, area);
    }

    fn render_controls(&self, frame: &mut Frame, area: Rect) {
        let title = self.selected_device().map_or_else(
            || " Controls ".to_string(),
            |device| format!(" Controls · {} ", device.name),
        );
        let block = Block::default()
            .title(Span::styled(
                title,
                Style::default().fg(NEON_CYAN).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BORDER_DIM));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width < 8 || inner.height == 0 {
            return;
        }

        if self.devices.is_empty() {
            frame.render_widget(
                Paragraph::new("No devices connected.").style(Style::default().fg(DIM_GRAY)),
                inner,
            );
            return;
        }

        if let Some(error) = &self.error {
            frame.render_widget(
                Paragraph::new(error.as_str())
                    .style(Style::default().fg(CORAL))
                    .wrap(Wrap { trim: false }),
                inner,
            );
            return;
        }

        if self.loading_device_id.is_some() && self.surfaces.is_empty() {
            frame.render_widget(
                Paragraph::new("Loading dynamic controls...").style(Style::default().fg(DIM_GRAY)),
                inner,
            );
            return;
        }

        if self.surfaces.is_empty() {
            frame.render_widget(
                Paragraph::new("No dynamic controls exposed.").style(Style::default().fg(DIM_GRAY)),
                inner,
            );
            return;
        }

        let selected = self.selected_control_key();
        let mut lines =
            control_surface_lines(&self.surfaces, usize::from(inner.width), selected.as_ref());
        if lines.is_empty() {
            frame.render_widget(
                Paragraph::new("No dynamic controls exposed.").style(Style::default().fg(DIM_GRAY)),
                inner,
            );
            return;
        }
        if let Some(notice) = &self.confirmation_notice {
            lines.insert(0, Line::default());
            lines.insert(
                0,
                Line::from(vec![
                    Span::styled("  Confirm  ", Style::default().fg(ELECTRIC_YELLOW)),
                    Span::styled(notice.clone(), Style::default().fg(BASE_WHITE)),
                ]),
            );
        }

        let max_scroll = lines.len().saturating_sub(usize::from(inner.height));
        let scroll = self.controls_scroll.get().min(max_scroll);
        self.controls_scroll.set(scroll);
        let visible_lines = lines
            .into_iter()
            .skip(scroll)
            .take(usize::from(inner.height))
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(visible_lines).wrap(Wrap { trim: false }),
            inner,
        );
    }

    fn visible_device_offset(&self, visible_rows: usize) -> usize {
        if visible_rows == 0 || self.devices.len() <= visible_rows {
            return 0;
        }
        let mut offset = self.devices_scroll.get();
        if self.selected_device < offset {
            offset = self.selected_device;
        } else if self.selected_device >= offset + visible_rows {
            offset = self
                .selected_device
                .saturating_sub(visible_rows.saturating_sub(1));
        }
        offset.min(self.devices.len().saturating_sub(visible_rows))
    }

    fn select_device(&mut self, index: usize) -> Option<Action> {
        if index >= self.devices.len() {
            return None;
        }
        self.selected_device = index;
        self.controls_scroll.set(0);
        self.selected_control = 0;
        self.clear_pending_confirmation();
        self.request_selected_controls()
    }

    fn move_device_down(&mut self, steps: usize) -> Option<Action> {
        if self.devices.is_empty() {
            return None;
        }
        let index = (self.selected_device + steps).min(self.devices.len().saturating_sub(1));
        self.select_device(index)
    }

    fn move_device_up(&mut self, steps: usize) -> Option<Action> {
        if self.devices.is_empty() {
            return None;
        }
        self.select_device(self.selected_device.saturating_sub(steps))
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
        let index = self.devices_scroll.get() + usize::from(row - first_row);
        (index < self.devices.len()).then_some(index)
    }

    fn scroll_controls_down(&self, steps: usize) {
        self.controls_scroll
            .set(self.controls_scroll.get().saturating_add(steps));
    }

    fn scroll_controls_up(&self, steps: usize) {
        self.controls_scroll
            .set(self.controls_scroll.get().saturating_sub(steps));
    }

    fn selected_control_key(&self) -> Option<ControlKey> {
        self.interactive_control_targets()
            .get(self.selected_control)
            .map(InteractiveControlTarget::key)
    }

    fn select_control_next(&mut self) -> Option<Action> {
        let count = self.interactive_control_targets().len();
        if count == 0 {
            return None;
        }
        self.selected_control = (self.selected_control + 1) % count;
        self.clear_pending_confirmation();
        Some(Action::Render)
    }

    fn select_control_prev(&mut self) -> Option<Action> {
        let count = self.interactive_control_targets().len();
        if count == 0 {
            return None;
        }
        self.selected_control = if self.selected_control == 0 {
            count - 1
        } else {
            self.selected_control - 1
        };
        self.clear_pending_confirmation();
        Some(Action::Render)
    }

    fn apply_selected_control_delta(&mut self, direction: i8) -> Option<Action> {
        let device_id = self.selected_device()?.id.clone();
        let target = self
            .interactive_control_targets()
            .get(self.selected_control)
            .cloned()?;
        let key = target.key();
        match target.kind {
            InteractiveControlKind::Field {
                field_id,
                value_type,
                value,
            } => {
                self.clear_pending_confirmation();
                let value = next_control_value(&value_type, &value, direction)?;
                Some(Action::ApplyDeviceControlChange {
                    device_id,
                    surface_id: target.surface_id,
                    expected_revision: target.revision,
                    field_id,
                    value,
                })
            }
            InteractiveControlKind::Action {
                action_id,
                input,
                confirmation_level,
                confirmation_message,
            } if direction >= 0 => {
                if let Some(message) = confirmation_message
                    && self.pending_confirmation.as_ref() != Some(&key)
                {
                    self.pending_confirmation = Some(key);
                    self.confirmation_notice =
                        Some(confirmation_notice(confirmation_level, &message));
                    self.controls_scroll.set(0);
                    return Some(Action::Render);
                }
                self.clear_pending_confirmation();
                Some(Action::InvokeDeviceControlAction {
                    device_id,
                    surface_id: target.surface_id,
                    action_id,
                    input,
                })
            }
            InteractiveControlKind::Action { .. } => {
                self.clear_pending_confirmation();
                None
            }
        }
    }

    fn interactive_control_targets(&self) -> Vec<InteractiveControlTarget> {
        self.surfaces
            .iter()
            .flat_map(|surface| {
                let fields = surface
                    .fields
                    .iter()
                    .filter(|field| {
                        field.access == ControlAccess::ReadWrite
                            && field_is_available(surface, field)
                            && surface.values.contains_key(&field.id)
                    })
                    .filter_map(|field| {
                        Some(InteractiveControlTarget {
                            surface_id: surface.surface_id.clone(),
                            revision: surface.revision,
                            kind: InteractiveControlKind::Field {
                                field_id: field.id.clone(),
                                value_type: field.value_type.clone(),
                                value: surface.values.get(&field.id)?.clone(),
                            },
                        })
                    });
                let actions = surface
                    .actions
                    .iter()
                    .filter(|action| action_is_available(surface, action))
                    .filter_map(|action| {
                        let input = default_action_input(action)?;
                        Some(InteractiveControlTarget {
                            surface_id: surface.surface_id.clone(),
                            revision: surface.revision,
                            kind: InteractiveControlKind::Action {
                                action_id: action.id.clone(),
                                input,
                                confirmation_level: action
                                    .confirmation
                                    .as_ref()
                                    .map(|confirmation| confirmation.level),
                                confirmation_message: action
                                    .confirmation
                                    .as_ref()
                                    .map(|confirmation| confirmation.message.clone()),
                            },
                        })
                    });
                fields.chain(actions).collect::<Vec<_>>()
            })
            .collect()
    }
}

impl Component for DeviceManagerView {
    fn init(&mut self, action_tx: UnboundedSender<Action>) -> Result<()> {
        self.action_tx = Some(action_tx);
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => Ok(self.move_device_down(1)),
            KeyCode::Char('k') | KeyCode::Up => Ok(self.move_device_up(1)),
            KeyCode::Tab => Ok(self.select_control_next()),
            KeyCode::BackTab => Ok(self.select_control_prev()),
            KeyCode::Char('h') | KeyCode::Left => Ok(self.apply_selected_control_delta(-1)),
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Char(' ') => {
                Ok(self.apply_selected_control_delta(1))
            }
            KeyCode::PageDown => {
                self.scroll_controls_down(5);
                Ok(Some(Action::Render))
            }
            KeyCode::PageUp => {
                self.scroll_controls_up(5);
                Ok(Some(Action::Render))
            }
            KeyCode::Home => {
                self.controls_scroll.set(0);
                Ok(Some(Action::Render))
            }
            KeyCode::End => {
                self.scroll_controls_down(usize::MAX / 2);
                Ok(Some(Action::Render))
            }
            KeyCode::Char('r') => Ok(self.request_selected_controls()),
            KeyCode::Enter => Ok(self
                .apply_selected_control_delta(1)
                .or_else(|| self.request_selected_controls())),
            _ => Ok(None),
        }
    }

    fn handle_mouse_event(&mut self, mouse: MouseEvent) -> Result<Option<Action>> {
        let col = mouse.column;
        let row = mouse.row;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = self.device_index_at(col, row) {
                    return Ok(self.select_device(index).or(Some(Action::Render)));
                }
            }
            MouseEventKind::ScrollDown if rect_contains(self.devices_rect.get(), col, row) => {
                return Ok(self.move_device_down(1).or(Some(Action::Render)));
            }
            MouseEventKind::ScrollUp if rect_contains(self.devices_rect.get(), col, row) => {
                return Ok(self.move_device_up(1).or(Some(Action::Render)));
            }
            MouseEventKind::ScrollDown if rect_contains(self.controls_rect.get(), col, row) => {
                self.scroll_controls_down(3);
                return Ok(Some(Action::Render));
            }
            MouseEventKind::ScrollUp if rect_contains(self.controls_rect.get(), col, row) => {
                self.scroll_controls_up(3);
                return Ok(Some(Action::Render));
            }
            _ => {}
        }
        Ok(None)
    }

    fn update(&mut self, action: &Action) -> Result<Option<Action>> {
        match action {
            Action::DevicesUpdated(devices) => {
                self.devices.clone_from(devices);
                if self.devices.is_empty() {
                    self.selected_device = 0;
                    self.surfaces.clear();
                    self.loaded_device_id = None;
                    self.devices_scroll.set(0);
                    self.controls_scroll.set(0);
                    return Ok(None);
                }
                self.selected_device = self
                    .selected_device
                    .min(self.devices.len().saturating_sub(1));
                if self.loaded_device_id.is_none() {
                    return Ok(self.request_selected_controls());
                }
            }
            Action::DeviceControlSurfacesUpdated {
                device_id,
                surfaces,
            } if self
                .selected_device()
                .is_some_and(|device| &device.id == device_id) =>
            {
                self.loaded_device_id = Some(device_id.clone());
                self.loading_device_id = None;
                self.error = None;
                self.clear_pending_confirmation();
                self.surfaces.clone_from(surfaces);
                self.selected_control = self
                    .selected_control
                    .min(self.interactive_control_targets().len().saturating_sub(1));
                self.controls_scroll.set(0);
            }
            Action::DeviceControlSurfacesFailed { device_id, error }
                if self
                    .selected_device()
                    .is_some_and(|device| &device.id == device_id) =>
            {
                self.loaded_device_id = Some(device_id.clone());
                self.loading_device_id = None;
                self.surfaces.clear();
                self.selected_control = 0;
                self.error = Some(error.clone());
                self.clear_pending_confirmation();
                self.controls_scroll.set(0);
            }
            Action::DeviceControlChangeApplied {
                device_id,
                response,
            } => {
                if self
                    .selected_device()
                    .is_some_and(|device| &device.id == device_id)
                    && let Some(surface) = self
                        .surfaces
                        .iter_mut()
                        .find(|surface| surface.surface_id == response.surface_id)
                {
                    surface.revision = response.revision;
                    surface.values.clone_from(&response.values);
                    self.error = None;
                    self.clear_pending_confirmation();
                    self.selected_control = self
                        .selected_control
                        .min(self.interactive_control_targets().len().saturating_sub(1));
                }
            }
            Action::DeviceControlChangeFailed {
                device_id, error, ..
            } if self
                .selected_device()
                .is_some_and(|device| &device.id == device_id) =>
            {
                self.error = Some(error.clone());
                self.clear_pending_confirmation();
            }
            Action::DeviceControlActionInvoked { device_id, .. }
                if self
                    .selected_device()
                    .is_some_and(|device| &device.id == device_id) =>
            {
                self.error = None;
                self.clear_pending_confirmation();
            }
            Action::DeviceControlSurfaceRefreshed { device_id, surface }
                if self
                    .selected_device()
                    .is_some_and(|device| &device.id == device_id) =>
            {
                if let Some(existing) = self
                    .surfaces
                    .iter_mut()
                    .find(|existing| existing.surface_id == surface.surface_id)
                {
                    existing.clone_from(surface.as_ref());
                } else {
                    self.surfaces.push(surface.as_ref().clone());
                }
                self.error = None;
                self.clear_pending_confirmation();
                self.selected_control = self
                    .selected_control
                    .min(self.interactive_control_targets().len().saturating_sub(1));
            }
            Action::DeviceControlActionFailed {
                device_id, error, ..
            } if self
                .selected_device()
                .is_some_and(|device| &device.id == device_id) =>
            {
                self.error = Some(error.clone());
                self.clear_pending_confirmation();
            }
            _ => {}
        }
        Ok(None)
    }

    fn render(&self, frame: &mut Frame, area: Rect) {
        if area.width < 4 || area.height < 4 {
            return;
        }
        let [devices, controls] = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(36), Constraint::Percentage(64)])
            .areas(area);
        self.devices_rect.set(devices);
        self.controls_rect.set(controls);
        self.render_devices(frame, devices);
        self.render_controls(frame, controls);
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn id(&self) -> &'static str {
        "device-manager"
    }
}

#[derive(Clone)]
struct InteractiveControlTarget {
    surface_id: String,
    revision: u64,
    kind: InteractiveControlKind,
}

impl InteractiveControlTarget {
    fn key(&self) -> ControlKey {
        match &self.kind {
            InteractiveControlKind::Field { field_id, .. } => ControlKey {
                surface_id: self.surface_id.clone(),
                item_id: field_id.clone(),
                kind: ControlKeyKind::Field,
            },
            InteractiveControlKind::Action { action_id, .. } => ControlKey {
                surface_id: self.surface_id.clone(),
                item_id: action_id.clone(),
                kind: ControlKeyKind::Action,
            },
        }
    }
}

#[derive(Clone)]
enum InteractiveControlKind {
    Field {
        field_id: String,
        value_type: ControlValueType,
        value: ControlValue,
    },
    Action {
        action_id: String,
        input: ControlValueMap,
        confirmation_level: Option<ActionConfirmationLevel>,
        confirmation_message: Option<String>,
    },
}

#[derive(Clone, PartialEq, Eq)]
struct ControlKey {
    surface_id: String,
    item_id: String,
    kind: ControlKeyKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ControlKeyKind {
    Field,
    Action,
}

fn control_surface_lines(
    surfaces: &[ControlSurfaceDocument],
    width: usize,
    selected: Option<&ControlKey>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for surface in surfaces {
        if !surface_has_visible_items(surface) {
            continue;
        }
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::from(vec![
            Span::styled(surface_title(surface), Style::default().fg(NEON_CYAN)),
            Span::styled(
                format!("  rev {}", surface.revision),
                Style::default().fg(DIM_GRAY),
            ),
        ]));

        let mut groups = surface.groups.clone();
        groups.sort_by_key(|group| group.ordering);
        for group in groups {
            append_group_lines(surface, &group, width, selected, &mut lines);
        }
        append_ungrouped_lines(surface, width, selected, &mut lines);
    }
    lines
}

fn surface_has_visible_items(surface: &ControlSurfaceDocument) -> bool {
    surface
        .fields
        .iter()
        .any(|field| !field_is_hidden(surface, field))
        || surface
            .actions
            .iter()
            .any(|action| !action_is_hidden(surface, action))
}

fn append_group_lines(
    surface: &ControlSurfaceDocument,
    group: &ControlGroupDescriptor,
    width: usize,
    selected: Option<&ControlKey>,
    lines: &mut Vec<Line<'static>>,
) {
    let fields = fields_for_group(surface, Some(&group.id));
    let actions = actions_for_group(surface, Some(&group.id));
    if fields.is_empty() && actions.is_empty() {
        return;
    }

    lines.push(Line::from(Span::styled(
        format!("  {}", group.label),
        Style::default()
            .fg(ELECTRIC_PURPLE)
            .add_modifier(Modifier::BOLD),
    )));
    append_items(surface, fields, actions, width, selected, lines);
}

fn append_ungrouped_lines(
    surface: &ControlSurfaceDocument,
    width: usize,
    selected: Option<&ControlKey>,
    lines: &mut Vec<Line<'static>>,
) {
    let known_groups = surface
        .groups
        .iter()
        .map(|group| group.id.as_str())
        .collect::<Vec<_>>();
    let fields = fields_without_known_group(surface, &known_groups);
    let actions = actions_without_known_group(surface, &known_groups);
    append_items(surface, fields, actions, width, selected, lines);
}

fn append_items(
    surface: &ControlSurfaceDocument,
    fields: Vec<&ControlFieldDescriptor>,
    actions: Vec<&ControlActionDescriptor>,
    width: usize,
    selected: Option<&ControlKey>,
    lines: &mut Vec<Line<'static>>,
) {
    for field in fields {
        let key = ControlKey {
            surface_id: surface.surface_id.clone(),
            item_id: field.id.clone(),
            kind: ControlKeyKind::Field,
        };
        let selected = selected.is_some_and(|selected| selected == &key);
        let marker = if selected { "\u{25B8} " } else { "  " };
        let label_style = if selected {
            Style::default().fg(NEON_CYAN).add_modifier(Modifier::BOLD)
        } else if field.access == ControlAccess::ReadWrite {
            Style::default().fg(BASE_WHITE).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(BASE_WHITE)
        };
        let value = surface
            .values
            .get(&field.id)
            .map_or_else(|| "-".to_string(), control_value_summary);
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(marker, Style::default().fg(NEON_CYAN)),
            Span::styled(
                truncate(&field.label, width.saturating_sub(18)),
                label_style,
            ),
            Span::styled("  ", Style::default()),
            Span::styled(value, Style::default().fg(ELECTRIC_YELLOW)),
        ]));
    }

    for action in actions {
        let key = ControlKey {
            surface_id: surface.surface_id.clone(),
            item_id: action.id.clone(),
            kind: ControlKeyKind::Action,
        };
        let selected = selected.is_some_and(|selected| selected == &key);
        let marker = if selected { "\u{25B8} " } else { "  " };
        let style = if selected {
            Style::default().fg(NEON_CYAN).add_modifier(Modifier::BOLD)
        } else if action.confirmation.is_some() {
            Style::default()
                .fg(ELECTRIC_YELLOW)
                .add_modifier(Modifier::BOLD)
        } else if action.input_fields.is_empty() {
            Style::default().fg(BASE_WHITE).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(BASE_WHITE)
        };
        let action_marker = match action
            .confirmation
            .as_ref()
            .map(|confirmation| confirmation.level)
        {
            Some(ActionConfirmationLevel::Destructive) => "! ",
            Some(ActionConfirmationLevel::HardwarePersistent | ActionConfirmationLevel::Normal) => {
                "? "
            }
            None => "\u{25B9} ",
        };
        let marker_style = if action.confirmation.is_some() {
            Style::default().fg(ELECTRIC_YELLOW)
        } else {
            Style::default().fg(CORAL)
        };
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(marker, Style::default().fg(NEON_CYAN)),
            Span::styled(action_marker, marker_style),
            Span::styled(action.label.clone(), style),
        ]));
    }
}

fn fields_for_group<'a>(
    surface: &'a ControlSurfaceDocument,
    group_id: Option<&str>,
) -> Vec<&'a ControlFieldDescriptor> {
    let mut fields = surface
        .fields
        .iter()
        .filter(|field| field.group_id.as_deref() == group_id)
        .filter(|field| !field_is_hidden(surface, field))
        .collect::<Vec<_>>();
    fields.sort_by_key(|field| field.ordering);
    fields
}

fn actions_for_group<'a>(
    surface: &'a ControlSurfaceDocument,
    group_id: Option<&str>,
) -> Vec<&'a ControlActionDescriptor> {
    let mut actions = surface
        .actions
        .iter()
        .filter(|action| action.group_id.as_deref() == group_id)
        .filter(|action| !action_is_hidden(surface, action))
        .collect::<Vec<_>>();
    actions.sort_by_key(|action| action.ordering);
    actions
}

fn fields_without_known_group<'a>(
    surface: &'a ControlSurfaceDocument,
    known_groups: &[&str],
) -> Vec<&'a ControlFieldDescriptor> {
    let mut fields = surface
        .fields
        .iter()
        .filter(|field| {
            field
                .group_id
                .as_deref()
                .is_none_or(|group_id| !known_groups.contains(&group_id))
        })
        .filter(|field| !field_is_hidden(surface, field))
        .collect::<Vec<_>>();
    fields.sort_by_key(|field| field.ordering);
    fields
}

fn actions_without_known_group<'a>(
    surface: &'a ControlSurfaceDocument,
    known_groups: &[&str],
) -> Vec<&'a ControlActionDescriptor> {
    let mut actions = surface
        .actions
        .iter()
        .filter(|action| {
            action
                .group_id
                .as_deref()
                .is_none_or(|group_id| !known_groups.contains(&group_id))
        })
        .filter(|action| !action_is_hidden(surface, action))
        .collect::<Vec<_>>();
    actions.sort_by_key(|action| action.ordering);
    actions
}

fn field_is_hidden(surface: &ControlSurfaceDocument, field: &ControlFieldDescriptor) -> bool {
    surface
        .availability
        .get(&field.id)
        .is_some_and(|availability| availability.state == ControlAvailabilityState::Hidden)
}

fn field_is_available(surface: &ControlSurfaceDocument, field: &ControlFieldDescriptor) -> bool {
    surface
        .availability
        .get(&field.id)
        .is_none_or(|availability| availability.state == ControlAvailabilityState::Available)
}

fn action_is_hidden(surface: &ControlSurfaceDocument, action: &ControlActionDescriptor) -> bool {
    surface
        .action_availability
        .get(&action.id)
        .is_some_and(|availability| availability.state == ControlAvailabilityState::Hidden)
}

fn action_is_available(surface: &ControlSurfaceDocument, action: &ControlActionDescriptor) -> bool {
    surface
        .action_availability
        .get(&action.id)
        .is_none_or(|availability| availability.state == ControlAvailabilityState::Available)
}

fn default_action_input(action: &ControlActionDescriptor) -> Option<ControlValueMap> {
    let mut input = ControlValueMap::new();
    for field in &action.input_fields {
        if let Some(value) = field.default_value.clone() {
            input.insert(field.id.clone(), value);
        } else if field.required {
            return None;
        }
    }
    Some(input)
}

fn confirmation_notice(level: Option<ActionConfirmationLevel>, message: &str) -> String {
    let prefix = match level {
        Some(ActionConfirmationLevel::Destructive) => "Destructive action",
        Some(ActionConfirmationLevel::HardwarePersistent) => "Hardware write",
        Some(ActionConfirmationLevel::Normal) | None => "Action",
    };
    format!("{prefix}: {message}. Press Enter again to run.")
}

fn surface_title(surface: &ControlSurfaceDocument) -> String {
    match &surface.scope {
        ControlSurfaceScope::Driver { driver_id } => format!("Driver · {driver_id}"),
        ControlSurfaceScope::Device {
            device_id,
            driver_id,
        } => format!("{driver_id} · {device_id}"),
    }
}

fn control_value_summary(value: &ControlValue) -> String {
    match value {
        ControlValue::Null => "-".to_string(),
        ControlValue::Bool(value) => value.to_string(),
        ControlValue::Integer(value) => value.to_string(),
        ControlValue::Float(value) => format!("{value:.2}"),
        ControlValue::String(value)
        | ControlValue::IpAddress(value)
        | ControlValue::MacAddress(value)
        | ControlValue::Enum(value) => value.clone(),
        ControlValue::SecretRef(_) => "configured".to_owned(),
        ControlValue::ColorRgb(value) => {
            format!("#{:02x}{:02x}{:02x}", value[0], value[1], value[2])
        }
        ControlValue::ColorRgba(value) => format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            value[0], value[1], value[2], value[3]
        ),
        ControlValue::DurationMs(value) => format!("{value}ms"),
        ControlValue::Flags(values) => values.join(", "),
        ControlValue::List(values) => format!("{} items", values.len()),
        ControlValue::Object(values) => format!("{} fields", values.len()),
        ControlValue::Unknown => "unsupported value".to_owned(),
    }
}

#[allow(clippy::cast_precision_loss)]
fn next_control_value(
    value_type: &ControlValueType,
    current: &ControlValue,
    direction: i8,
) -> Option<ControlValue> {
    match (value_type, current) {
        (ControlValueType::Bool, ControlValue::Bool(value)) => Some(ControlValue::Bool(!value)),
        (ControlValueType::Integer { min, max, step }, ControlValue::Integer(value)) => {
            let delta = step.unwrap_or(1).abs().max(1);
            let next = if direction < 0 {
                value.saturating_sub(delta)
            } else {
                value.saturating_add(delta)
            };
            Some(ControlValue::Integer(clamp_i64(next, *min, *max)))
        }
        (ControlValueType::Float { min, max, step }, ControlValue::Float(value)) => {
            let delta = step.unwrap_or(1.0).abs().max(f64::EPSILON);
            let next = if direction < 0 {
                value - delta
            } else {
                value + delta
            };
            Some(ControlValue::Float(clamp_f64(next, *min, *max)))
        }
        (ControlValueType::Enum { options }, ControlValue::Enum(value)) => {
            if options.is_empty() {
                return None;
            }
            let current = options
                .iter()
                .position(|option| option.value == *value)
                .unwrap_or_default();
            let next = if direction < 0 {
                current.checked_sub(1).unwrap_or(options.len() - 1)
            } else {
                (current + 1) % options.len()
            };
            Some(ControlValue::Enum(options[next].value.clone()))
        }
        _ => None,
    }
}

fn clamp_i64(value: i64, min: Option<i64>, max: Option<i64>) -> i64 {
    let value = min.map_or(value, |min| value.max(min));
    max.map_or(value, |max| value.min(max))
}

fn clamp_f64(value: f64, min: Option<f64>, max: Option<f64>) -> f64 {
    let value = min.map_or(value, |min| value.max(min));
    max.map_or(value, |max| value.min(max))
}

fn truncate(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        value.to_string()
    } else if max_len > 1 {
        format!("{}\u{2026}", &value[..max_len - 1])
    } else {
        "\u{2026}".to_string()
    }
}

fn rect_contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

#[cfg(test)]
mod tests {
    use hypercolor_types::controls::ControlValue;

    use super::control_value_summary;

    #[test]
    fn control_value_summary_hides_secret_refs() {
        assert_eq!(
            control_value_summary(&ControlValue::SecretRef("driver-owned-secret".to_owned())),
            "configured"
        );
    }

    #[test]
    fn control_value_summary_marks_unknown_values_unsupported() {
        assert_eq!(
            control_value_summary(&ControlValue::Unknown),
            "unsupported value"
        );
    }
}
