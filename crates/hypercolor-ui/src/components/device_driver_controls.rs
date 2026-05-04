use std::collections::BTreeMap;

use hypercolor_leptos_ext::events::Change;
use hypercolor_types::controls::{
    ActionConfirmationLevel, ControlAccess, ControlActionDescriptor, ControlAvailabilityState,
    ControlChange, ControlFieldDescriptor, ControlGroupDescriptor, ControlSurfaceDocument,
    ControlSurfaceScope, ControlValue as DynamicControlValue, ControlValueMap, ControlValueType,
};
use leptos::prelude::*;
use leptos_icons::Icon;

use hypercolor_ui::control_surface_view::{
    action_is_hidden, actionable_control_surfaces, control_value_summary,
    driver_owned_device_control_surfaces, field_is_hidden,
};

use crate::api;
use crate::app::WsContext;
use crate::components::device_card::driver_identifier_label;
use crate::icons::*;
use crate::toasts;

#[component]
pub fn DeviceDriverControls(#[prop(into)] device_id: Signal<String>) -> impl IntoView {
    let ws_ctx = expect_context::<WsContext>();
    let surfaces_resource = LocalResource::new(move || {
        let id = device_id.get();
        async move { api::fetch_device_control_surfaces(&id, false).await }
    });
    let surface_overrides = RwSignal::new(BTreeMap::<String, ControlSurfaceDocument>::new());

    Effect::new(move |_| {
        let Some(event) = ws_ctx.last_control_surface_event.get() else {
            return;
        };
        let current_device_id = device_id.get_untracked();
        if !driver_surface_event_matches_device(&event.surface_id, &current_device_id) {
            return;
        }

        let surface_id = event.surface_id.clone();
        leptos::task::spawn_local(async move {
            match api::fetch_control_surface(&surface_id).await {
                Ok(surface) => {
                    surface_overrides.update(|overrides| {
                        overrides.insert(surface.surface_id.clone(), surface);
                    });
                }
                Err(error) => {
                    leptos::logging::warn!("Driver control surface refresh failed: {error}");
                }
            }
        });
    });

    view! {
        {move || match surfaces_resource.get() {
            Some(Ok(surfaces)) => {
                let current_device_id = device_id.get();
                let surfaces = merge_control_surface_overrides(
                    surfaces,
                    surface_overrides.get(),
                    &current_device_id,
                );
                let surfaces = actionable_control_surfaces(driver_owned_device_control_surfaces(
                    surfaces,
                    &current_device_id,
                ));
                if surfaces.is_empty() {
                    ().into_any()
                } else {
                    let show_surface_titles = surfaces.len() > 1;
                    view! {
                        <div class="rounded-xl bg-surface-raised border border-edge-subtle/60 overflow-hidden">
                            <div class="px-4 py-3 border-b border-edge-subtle/60 flex items-center justify-between">
                                <div class="flex items-center gap-2 min-w-0">
                                    <Icon icon=LuSlidersHorizontal width="13px" height="13px" style="color: rgba(128, 255, 234, 0.72)" />
                                    <h3 class="text-[11px] font-medium text-fg-secondary truncate">"Driver Controls"</h3>
                                </div>
                                <button
                                    class="w-6 h-6 rounded-md flex items-center justify-center text-fg-tertiary/55 hover:text-fg-secondary hover:bg-surface-hover/40 transition-colors"
                                    title="Refresh"
                                    on:click=move |_| {
                                        surface_overrides.update(BTreeMap::clear);
                                        surfaces_resource.refetch();
                                    }
                                >
                                    <Icon icon=LuRefreshCw width="12px" height="12px" />
                                </button>
                            </div>
                            <div class="divide-y divide-edge-subtle/35">
                                {surfaces.into_iter().map(|surface| {
                                    render_surface(surface, show_surface_titles, surfaces_resource)
                                }).collect_view()}
                            </div>
                        </div>
                    }.into_any()
                }
            }
            Some(Err(error)) => view! {
                <div class="rounded-xl bg-surface-raised border border-edge-subtle/60 overflow-hidden">
                    <div class="px-4 py-3 flex items-center gap-2 text-[10px] text-fg-tertiary/60">
                        <Icon icon=LuTriangleAlert width="12px" height="12px" />
                        <span>{error}</span>
                    </div>
                </div>
            }.into_any(),
            None => ().into_any(),
        }}
    }
}

fn merge_control_surface_overrides(
    mut surfaces: Vec<ControlSurfaceDocument>,
    overrides: BTreeMap<String, ControlSurfaceDocument>,
    device_id: &str,
) -> Vec<ControlSurfaceDocument> {
    for surface in &mut surfaces {
        if let Some(fresh) = overrides.get(&surface.surface_id) {
            *surface = fresh.clone();
        }
    }

    for surface in overrides.into_values() {
        let already_listed = surfaces
            .iter()
            .any(|listed| listed.surface_id == surface.surface_id);
        if !already_listed && driver_surface_event_matches_device(&surface.surface_id, device_id) {
            surfaces.push(surface);
        }
    }

    surfaces
}

fn driver_surface_event_matches_device(surface_id: &str, device_id: &str) -> bool {
    surface_id.ends_with(&format!(":device:{device_id}"))
}

fn render_surface(
    surface: ControlSurfaceDocument,
    show_title: bool,
    surfaces_resource: LocalResource<Result<Vec<ControlSurfaceDocument>, String>>,
) -> impl IntoView {
    let title = surface_title(&surface);
    let groups = grouped_surface_items(&surface);

    view! {
        <section>
            {show_title.then(|| view! {
                <div class="px-4 py-2 border-b border-edge-subtle/35">
                    <div class="flex items-center justify-between gap-2">
                        <div class="text-[10px] font-mono font-semibold tracking-[0.14em] uppercase text-fg-tertiary/70 truncate">
                            {title}
                        </div>
                        <div class="text-[9px] font-mono text-fg-tertiary/35 tabular-nums">
                            {"rev "}{surface.revision}
                        </div>
                    </div>
                </div>
            })}
            <div class="divide-y divide-edge-subtle/30">
                {groups.into_iter().map(|group| {
                    render_group(surface.clone(), group, surfaces_resource)
                }).collect_view()}
            </div>
        </section>
    }
}

#[derive(Clone)]
struct ControlSurfaceSection {
    id: Option<String>,
    label: Option<String>,
    ordering: i32,
    items: Vec<ControlSurfaceItem>,
}

#[derive(Clone)]
enum ControlSurfaceItem {
    Field(ControlFieldDescriptor),
    Action(ControlActionDescriptor),
}

impl ControlSurfaceItem {
    fn ordering(&self) -> i32 {
        match self {
            Self::Field(field) => field.ordering,
            Self::Action(action) => action.ordering,
        }
    }

    fn group_id(&self) -> Option<&str> {
        match self {
            Self::Field(field) => field.group_id.as_deref(),
            Self::Action(action) => action.group_id.as_deref(),
        }
    }
}

fn grouped_surface_items(surface: &ControlSurfaceDocument) -> Vec<ControlSurfaceSection> {
    let mut sections = surface
        .groups
        .iter()
        .cloned()
        .map(section_from_group)
        .collect::<Vec<_>>();
    sections.sort_by(|left, right| {
        left.ordering
            .cmp(&right.ordering)
            .then_with(|| left.label.cmp(&right.label))
    });

    let mut ungrouped = ControlSurfaceSection {
        id: None,
        label: None,
        ordering: i32::MAX,
        items: Vec::new(),
    };
    let mut items = surface
        .fields
        .iter()
        .filter(|field| !field_is_hidden(surface, field))
        .cloned()
        .map(ControlSurfaceItem::Field)
        .chain(
            surface
                .actions
                .iter()
                .filter(|action| !action_is_hidden(surface, action))
                .cloned()
                .map(ControlSurfaceItem::Action),
        )
        .collect::<Vec<_>>();
    items.sort_by_key(ControlSurfaceItem::ordering);

    for item in items {
        let Some(group_id) = item.group_id() else {
            ungrouped.items.push(item);
            continue;
        };
        if let Some(section) = sections
            .iter_mut()
            .find(|section| section.id.as_deref() == Some(group_id))
        {
            section.items.push(item);
        } else {
            ungrouped.items.push(item);
        }
    }

    sections.retain(|section| !section.items.is_empty());
    if !ungrouped.items.is_empty() {
        sections.push(ungrouped);
    }
    sections
}

fn section_from_group(group: ControlGroupDescriptor) -> ControlSurfaceSection {
    ControlSurfaceSection {
        id: Some(group.id),
        label: Some(group.label),
        ordering: group.ordering,
        items: Vec::new(),
    }
}

fn render_group(
    surface: ControlSurfaceDocument,
    group: ControlSurfaceSection,
    surfaces_resource: LocalResource<Result<Vec<ControlSurfaceDocument>, String>>,
) -> impl IntoView {
    let label = group.label.clone();
    let items = group.items.clone();

    view! {
        <section>
            {label.map(|label| view! {
                <div class="px-4 pt-3 pb-1.5 bg-surface-sunken/10">
                    <div class="text-[9px] font-semibold uppercase tracking-[0.12em] text-fg-tertiary/65">
                        {label}
                    </div>
                </div>
            })}
            <div class="divide-y divide-edge-subtle/25">
                {items.into_iter().map(|item| match item {
                    ControlSurfaceItem::Field(field) => {
                        render_field(surface.clone(), field, surfaces_resource).into_any()
                    }
                    ControlSurfaceItem::Action(action) => {
                        render_action(surface.clone(), action, surfaces_resource).into_any()
                    }
                }).collect_view()}
            </div>
        </section>
    }
}

fn render_field(
    surface: ControlSurfaceDocument,
    field: ControlFieldDescriptor,
    surfaces_resource: LocalResource<Result<Vec<ControlSurfaceDocument>, String>>,
) -> impl IntoView {
    let field_id = field.id.clone();
    let current_value = surface.values.get(&field_id).cloned();
    let availability = surface.availability.get(&field_id).cloned();
    let availability_state = availability
        .as_ref()
        .map(|availability| availability.state)
        .unwrap_or(ControlAvailabilityState::Available);
    let availability_reason = availability.and_then(|availability| availability.reason);
    let editable = field.access != ControlAccess::ReadOnly
        && availability_state == ControlAvailabilityState::Available;
    let field_label = field.label.clone();
    let description = field.description.clone();
    let value_view = render_field_editor(
        surface.surface_id.clone(),
        surface.revision,
        field.clone(),
        current_value,
        editable,
        surfaces_resource,
    );

    view! {
        <div class="px-4 py-2.5 grid grid-cols-[minmax(0,1fr)_minmax(8rem,12rem)] gap-3 items-center">
            <div class="min-w-0">
                <div class="text-[11px] text-fg-secondary font-medium truncate">{field_label}</div>
                {description.map(|text| view! {
                    <div class="text-[9px] text-fg-tertiary/45 leading-snug mt-0.5">{text}</div>
                })}
                {availability_reason.map(|text| view! {
                    <div class="text-[9px] text-yellow-200/65 leading-snug mt-0.5">{text}</div>
                })}
            </div>
            <div class="min-w-0 flex justify-end">{value_view}</div>
        </div>
    }
}

fn render_field_editor(
    surface_id: String,
    revision: u64,
    field: ControlFieldDescriptor,
    current_value: Option<DynamicControlValue>,
    editable: bool,
    surfaces_resource: LocalResource<Result<Vec<ControlSurfaceDocument>, String>>,
) -> AnyView {
    if !editable {
        return render_read_only_value(current_value.as_ref()).into_any();
    }

    match &field.value_type {
        ControlValueType::Bool => render_bool_editor(
            surface_id,
            revision,
            field.id.clone(),
            matches!(current_value, Some(DynamicControlValue::Bool(true))),
            surfaces_resource,
        )
        .into_any(),
        ControlValueType::Integer { min, max, step } => render_number_editor(NumberEditorProps {
            kind: NumberEditorKind::Integer,
            surface_id,
            revision,
            field_id: field.id.clone(),
            value: number_text(current_value.as_ref()),
            min: min.map(|value| value.to_string()),
            max: max.map(|value| value.to_string()),
            step: step.map_or_else(|| "1".to_string(), |value| value.to_string()),
            surfaces_resource,
        })
        .into_any(),
        ControlValueType::Float { min, max, step } => render_number_editor(NumberEditorProps {
            kind: NumberEditorKind::Float,
            surface_id,
            revision,
            field_id: field.id.clone(),
            value: number_text(current_value.as_ref()),
            min: min.map(|value| value.to_string()),
            max: max.map(|value| value.to_string()),
            step: step.map_or_else(|| "0.01".to_string(), |value| value.to_string()),
            surfaces_resource,
        })
        .into_any(),
        ControlValueType::DurationMs { min, max, step } => {
            render_number_editor(NumberEditorProps {
                kind: NumberEditorKind::DurationMs,
                surface_id,
                revision,
                field_id: field.id.clone(),
                value: number_text(current_value.as_ref()),
                min: min.map(|value| value.to_string()),
                max: max.map(|value| value.to_string()),
                step: step.map_or_else(|| "100".to_string(), |value| value.to_string()),
                surfaces_resource,
            })
            .into_any()
        }
        ControlValueType::Enum { options } => render_enum_editor(
            surface_id,
            revision,
            field.id.clone(),
            enum_text(current_value.as_ref()),
            options
                .iter()
                .map(|option| (option.value.clone(), option.label.clone()))
                .collect(),
            surfaces_resource,
        )
        .into_any(),
        ControlValueType::String { .. }
        | ControlValueType::Secret
        | ControlValueType::IpAddress
        | ControlValueType::MacAddress
        | ControlValueType::ColorRgb
        | ControlValueType::ColorRgba => render_text_editor(
            text_editor_kind(&field.value_type),
            surface_id,
            revision,
            field.id.clone(),
            value_text(current_value.as_ref()),
            surfaces_resource,
        )
        .into_any(),
        ControlValueType::Flags { .. }
        | ControlValueType::List { .. }
        | ControlValueType::Object { .. }
        | ControlValueType::Unknown => render_read_only_value(current_value.as_ref()).into_any(),
    }
}

fn render_read_only_value(current_value: Option<&DynamicControlValue>) -> impl IntoView {
    let summary = value_text(current_value);
    view! {
        <div class="max-w-full rounded-md border border-edge-subtle/45 bg-surface-sunken/70 px-2 py-1 text-right">
            <div class="truncate text-[10px] font-mono text-fg-tertiary/70">
                {if summary.is_empty() { "Unavailable".to_string() } else { summary }}
            </div>
        </div>
    }
}

fn render_bool_editor(
    surface_id: String,
    revision: u64,
    field_id: String,
    checked: bool,
    surfaces_resource: LocalResource<Result<Vec<ControlSurfaceDocument>, String>>,
) -> impl IntoView {
    view! {
        <input
            type="checkbox"
            class="w-4 h-4 rounded border-edge-subtle accent-cyan-300"
            prop:checked=checked
            on:change=move |ev| {
                if let Some(next) = Change::from_event(ev).checked() {
                    apply_change(
                        surface_id.clone(),
                        revision,
                        field_id.clone(),
                        DynamicControlValue::Bool(next),
                        surfaces_resource,
                    );
                }
            }
        />
    }
}

struct NumberEditorProps {
    kind: NumberEditorKind,
    surface_id: String,
    revision: u64,
    field_id: String,
    value: String,
    min: Option<String>,
    max: Option<String>,
    step: String,
    surfaces_resource: LocalResource<Result<Vec<ControlSurfaceDocument>, String>>,
}

#[derive(Clone, Copy)]
enum NumberEditorKind {
    Integer,
    Float,
    DurationMs,
}

fn render_number_editor(props: NumberEditorProps) -> impl IntoView {
    let NumberEditorProps {
        kind,
        surface_id,
        revision,
        field_id,
        value,
        min,
        max,
        step,
        surfaces_resource,
    } = props;

    view! {
        <input
            type="number"
            class="w-full max-w-36 bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1 text-[10px] font-mono text-fg-secondary
                   focus:outline-none focus:border-accent-muted"
            prop:value=value
            min=min
            max=max
            step=step
            on:change=move |ev| {
                let Some(raw) = Change::from_event(ev).value_string() else {
                    return;
                };
                let parsed = match parse_number_value(kind, &raw) {
                    Ok(value) => value,
                    Err(error) => {
                        toasts::toast_error(&error);
                        return;
                    }
                };
                apply_change(
                    surface_id.clone(),
                    revision,
                    field_id.clone(),
                    parsed,
                    surfaces_resource,
                );
            }
        />
    }
}

fn render_enum_editor(
    surface_id: String,
    revision: u64,
    field_id: String,
    value: String,
    options: Vec<(String, String)>,
    surfaces_resource: LocalResource<Result<Vec<ControlSurfaceDocument>, String>>,
) -> impl IntoView {
    view! {
        <select
            class="w-full max-w-40 bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1 text-[10px] text-fg-secondary
                   focus:outline-none focus:border-accent-muted"
            prop:value=value
            on:change=move |ev| {
                let Some(raw) = Change::from_event(ev).value_string() else {
                    return;
                };
                apply_change(
                    surface_id.clone(),
                    revision,
                    field_id.clone(),
                    DynamicControlValue::Enum(raw),
                    surfaces_resource,
                );
            }
        >
            {options.into_iter().map(|(value, label)| view! {
                <option value=value>{label}</option>
            }).collect_view()}
        </select>
    }
}

fn render_text_editor(
    kind: TextEditorKind,
    surface_id: String,
    revision: u64,
    field_id: String,
    value: String,
    surfaces_resource: LocalResource<Result<Vec<ControlSurfaceDocument>, String>>,
) -> impl IntoView {
    let is_secret = matches!(kind, TextEditorKind::Secret);
    view! {
        <input
            type=if is_secret { "password" } else { "text" }
            class="w-full max-w-44 bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1 text-[10px] font-mono text-fg-secondary
                   focus:outline-none focus:border-accent-muted"
            prop:value=if is_secret { String::new() } else { value }
            placeholder=if is_secret { "Configured" } else { "" }
            on:change=move |ev| {
                let Some(raw) = Change::from_event(ev).value_string() else {
                    return;
                };
                apply_change(
                    surface_id.clone(),
                    revision,
                    field_id.clone(),
                    text_value(kind, raw),
                    surfaces_resource,
                );
            }
        />
    }
}

fn render_action(
    surface: ControlSurfaceDocument,
    action: ControlActionDescriptor,
    surfaces_resource: LocalResource<Result<Vec<ControlSurfaceDocument>, String>>,
) -> impl IntoView {
    let action_id = action.id.clone();
    let state = surface
        .action_availability
        .get(&action_id)
        .map(|availability| availability.state)
        .unwrap_or(ControlAvailabilityState::Available);
    let enabled = state == ControlAvailabilityState::Available && action.input_fields.is_empty();
    let confirmation_level = action
        .confirmation
        .as_ref()
        .map(|confirmation| confirmation.level);
    let confirmation_message = action
        .confirmation
        .as_ref()
        .map(|confirmation| confirmation.message.clone());
    let (confirmation_armed, set_confirmation_armed) = signal(false);
    let surface_id = surface.surface_id.clone();
    let action_label = action.label.clone();
    let description = action.description.clone();

    view! {
        <div class="px-4 py-2.5 grid grid-cols-[minmax(0,1fr)_auto] gap-3 items-center">
            <div class="min-w-0">
                <div class="text-[11px] text-fg-secondary font-medium truncate">{action_label}</div>
                {description.map(|text| view! {
                    <div class="text-[9px] text-fg-tertiary/45 leading-snug mt-0.5">{text}</div>
                })}
            </div>
            <button
                class=move || action_button_class(enabled, confirmation_level, confirmation_armed.get())
                disabled=!enabled
                on:click=move |_| {
                    if let Some(message) = confirmation_message.clone()
                        && !confirmation_armed.get_untracked()
                    {
                        set_confirmation_armed.set(true);
                        toasts::toast_info(&message);
                        return;
                    }
                    let surface_id = surface_id.clone();
                    let action_id = action_id.clone();
                    leptos::task::spawn_local(async move {
                        match api::invoke_control_action(&surface_id, &action_id, ControlValueMap::new()).await {
                            Ok(_) => {
                                set_confirmation_armed.set(false);
                                toasts::toast_success("Action sent");
                                surfaces_resource.refetch();
                            }
                            Err(error) => {
                                set_confirmation_armed.set(false);
                                toasts::toast_error(&format!("Action failed: {error}"));
                            }
                        }
                    });
                }
            >
                <Icon icon=if confirmation_level == Some(ActionConfirmationLevel::Destructive) { LuTriangleAlert } else { LuPlay } width="10px" height="10px" />
                {move || if confirmation_armed.get() { "Confirm" } else { "Run" }}
            </button>
        </div>
    }
}

fn action_button_class(
    enabled: bool,
    confirmation_level: Option<ActionConfirmationLevel>,
    armed: bool,
) -> &'static str {
    if !enabled {
        return "px-2 py-1 rounded-md text-[10px] font-medium transition-all flex items-center gap-1 bg-surface-hover/35 text-fg-tertiary/45 disabled:pointer-events-none";
    }

    match (confirmation_level, armed) {
        (Some(ActionConfirmationLevel::Destructive), true) => {
            "px-2 py-1 rounded-md text-[10px] font-medium transition-all btn-press flex items-center gap-1 bg-red-500/15 text-red-300 hover:text-red-200"
        }
        (Some(_), true) | (Some(ActionConfirmationLevel::Destructive), false) => {
            "px-2 py-1 rounded-md text-[10px] font-medium transition-all btn-press flex items-center gap-1 bg-yellow-500/10 text-yellow-200 hover:text-yellow-100"
        }
        _ => {
            "px-2 py-1 rounded-md text-[10px] font-medium transition-all btn-press flex items-center gap-1 bg-surface-hover/45 text-fg-secondary hover:text-accent"
        }
    }
}

fn apply_change(
    surface_id: String,
    revision: u64,
    field_id: String,
    value: DynamicControlValue,
    surfaces_resource: LocalResource<Result<Vec<ControlSurfaceDocument>, String>>,
) {
    leptos::task::spawn_local(async move {
        let change = ControlChange { field_id, value };
        match api::patch_control_values(surface_id, Some(revision), vec![change], false).await {
            Ok(_) => {
                toasts::toast_success("Driver control updated");
                surfaces_resource.refetch();
            }
            Err(error) => {
                toasts::toast_error(&format!("Driver control failed: {error}"));
                surfaces_resource.refetch();
            }
        }
    });
}

fn parse_number_value(kind: NumberEditorKind, raw: &str) -> Result<DynamicControlValue, String> {
    match kind {
        NumberEditorKind::Integer => raw
            .parse::<i64>()
            .map(DynamicControlValue::Integer)
            .map_err(|_| "Expected an integer value".to_string()),
        NumberEditorKind::Float => raw
            .parse::<f64>()
            .map(DynamicControlValue::Float)
            .map_err(|_| "Expected a number".to_string()),
        NumberEditorKind::DurationMs => raw
            .parse::<u64>()
            .map(DynamicControlValue::DurationMs)
            .map_err(|_| "Expected a duration in milliseconds".to_string()),
    }
}

#[derive(Clone, Copy)]
enum TextEditorKind {
    String,
    Secret,
    IpAddress,
    MacAddress,
    ColorRgb,
    ColorRgba,
}

fn text_editor_kind(value_type: &ControlValueType) -> TextEditorKind {
    match value_type {
        ControlValueType::Secret => TextEditorKind::Secret,
        ControlValueType::IpAddress => TextEditorKind::IpAddress,
        ControlValueType::MacAddress => TextEditorKind::MacAddress,
        ControlValueType::ColorRgb => TextEditorKind::ColorRgb,
        ControlValueType::ColorRgba => TextEditorKind::ColorRgba,
        _ => TextEditorKind::String,
    }
}

fn text_value(kind: TextEditorKind, raw: String) -> DynamicControlValue {
    match kind {
        TextEditorKind::String => DynamicControlValue::String(raw),
        TextEditorKind::Secret => DynamicControlValue::SecretRef(raw),
        TextEditorKind::IpAddress => DynamicControlValue::IpAddress(raw),
        TextEditorKind::MacAddress => DynamicControlValue::MacAddress(raw),
        TextEditorKind::ColorRgb => DynamicControlValue::ColorRgb(parse_color_rgb(&raw)),
        TextEditorKind::ColorRgba => DynamicControlValue::ColorRgba(parse_color_rgba(&raw)),
    }
}

fn parse_color_rgb(raw: &str) -> [u8; 3] {
    let bytes = parse_color_bytes(raw);
    [bytes[0], bytes[1], bytes[2]]
}

fn parse_color_rgba(raw: &str) -> [u8; 4] {
    let bytes = parse_color_bytes(raw);
    [bytes[0], bytes[1], bytes[2], bytes[3]]
}

fn parse_color_bytes(raw: &str) -> [u8; 4] {
    let hex = raw.trim().trim_start_matches('#');
    let mut out = [0_u8, 0_u8, 0_u8, 255_u8];
    for (index, channel) in out.iter_mut().enumerate() {
        let start = index * 2;
        let end = start + 2;
        if end <= hex.len()
            && let Ok(byte) = u8::from_str_radix(&hex[start..end], 16)
        {
            *channel = byte;
        }
    }
    out
}

fn number_text(value: Option<&DynamicControlValue>) -> String {
    match value {
        Some(DynamicControlValue::Integer(value)) => value.to_string(),
        Some(DynamicControlValue::Float(value)) => value.to_string(),
        Some(DynamicControlValue::DurationMs(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn enum_text(value: Option<&DynamicControlValue>) -> String {
    match value {
        Some(DynamicControlValue::Enum(value)) => value.clone(),
        _ => String::new(),
    }
}

fn value_text(value: Option<&DynamicControlValue>) -> String {
    control_value_summary(value)
}

fn surface_title(surface: &ControlSurfaceDocument) -> String {
    match &surface.scope {
        ControlSurfaceScope::Driver { driver_id }
        | ControlSurfaceScope::Device { driver_id, .. } => {
            driver_identifier_label(driver_id).unwrap_or_else(|| driver_id.to_uppercase())
        }
    }
}
