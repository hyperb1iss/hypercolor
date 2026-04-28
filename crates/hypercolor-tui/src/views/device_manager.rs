//! Device manager view for generic dynamic control surfaces.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use hypercolor_types::controls::{
    ControlActionDescriptor, ControlAvailabilityState, ControlFieldDescriptor,
    ControlGroupDescriptor, ControlSurfaceDocument, ControlSurfaceScope, ControlValue,
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
    loaded_device_id: Option<String>,
    surfaces: Vec<ControlSurfaceDocument>,
    loading_device_id: Option<String>,
    error: Option<String>,
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
            loaded_device_id: None,
            surfaces: Vec::new(),
            loading_device_id: None,
            error: None,
        }
    }

    fn selected_device(&self) -> Option<&DeviceSummary> {
        self.devices.get(self.selected_device)
    }

    fn request_selected_controls(&mut self) -> Option<Action> {
        let device_id = self.selected_device()?.id.clone();
        self.loading_device_id = Some(device_id.clone());
        self.error = None;
        Some(Action::LoadDeviceControls(device_id))
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

        let rows = self.devices.iter().enumerate().map(|(index, device)| {
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

        let lines = control_surface_lines(&self.surfaces, usize::from(inner.width));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }
}

impl Component for DeviceManagerView {
    fn init(&mut self, action_tx: UnboundedSender<Action>) -> Result<()> {
        self.action_tx = Some(action_tx);
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.devices.is_empty() {
                    self.selected_device =
                        (self.selected_device + 1).min(self.devices.len().saturating_sub(1));
                    return Ok(self.request_selected_controls());
                }
                Ok(None)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if !self.devices.is_empty() {
                    self.selected_device = self.selected_device.saturating_sub(1);
                    return Ok(self.request_selected_controls());
                }
                Ok(None)
            }
            KeyCode::Char('r') | KeyCode::Enter => Ok(self.request_selected_controls()),
            _ => Ok(None),
        }
    }

    fn handle_mouse_event(&mut self, _mouse: MouseEvent) -> Result<Option<Action>> {
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
            } => {
                if self
                    .selected_device()
                    .is_some_and(|device| &device.id == device_id)
                {
                    self.loaded_device_id = Some(device_id.clone());
                    self.loading_device_id = None;
                    self.error = None;
                    self.surfaces.clone_from(surfaces);
                }
            }
            Action::DeviceControlSurfacesFailed { device_id, error } => {
                if self
                    .selected_device()
                    .is_some_and(|device| &device.id == device_id)
                {
                    self.loaded_device_id = Some(device_id.clone());
                    self.loading_device_id = None;
                    self.surfaces.clear();
                    self.error = Some(error.clone());
                }
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

fn control_surface_lines(surfaces: &[ControlSurfaceDocument], width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for surface in surfaces {
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
            append_group_lines(surface, &group, width, &mut lines);
        }
        append_ungrouped_lines(surface, width, &mut lines);
    }
    lines
}

fn append_group_lines(
    surface: &ControlSurfaceDocument,
    group: &ControlGroupDescriptor,
    width: usize,
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
    append_items(surface, fields, actions, width, lines);
}

fn append_ungrouped_lines(
    surface: &ControlSurfaceDocument,
    width: usize,
    lines: &mut Vec<Line<'static>>,
) {
    let known_groups = surface
        .groups
        .iter()
        .map(|group| group.id.as_str())
        .collect::<Vec<_>>();
    let fields = fields_without_known_group(surface, &known_groups);
    let actions = actions_without_known_group(surface, &known_groups);
    append_items(surface, fields, actions, width, lines);
}

fn append_items(
    surface: &ControlSurfaceDocument,
    fields: Vec<&ControlFieldDescriptor>,
    actions: Vec<&ControlActionDescriptor>,
    width: usize,
    lines: &mut Vec<Line<'static>>,
) {
    for field in fields {
        let value = surface
            .values
            .get(&field.id)
            .map_or_else(|| "-".to_string(), control_value_summary);
        lines.push(Line::from(vec![
            Span::styled("    ", Style::default()),
            Span::styled(
                truncate(&field.label, width.saturating_sub(18)),
                Style::default().fg(BASE_WHITE),
            ),
            Span::styled("  ", Style::default()),
            Span::styled(value, Style::default().fg(ELECTRIC_YELLOW)),
        ]));
    }

    for action in actions {
        lines.push(Line::from(vec![
            Span::styled("    \u{25B9} ", Style::default().fg(CORAL)),
            Span::styled(action.label.clone(), Style::default().fg(BASE_WHITE)),
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

fn action_is_hidden(surface: &ControlSurfaceDocument, action: &ControlActionDescriptor) -> bool {
    surface
        .action_availability
        .get(&action.id)
        .is_some_and(|availability| availability.state == ControlAvailabilityState::Hidden)
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
        | ControlValue::SecretRef(value)
        | ControlValue::IpAddress(value)
        | ControlValue::MacAddress(value)
        | ControlValue::Enum(value) => value.clone(),
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
    }
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
