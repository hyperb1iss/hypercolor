//! Generic dynamic controls for device and driver-owned surfaces.

use hypercolor_leptos_ext::events::Change;
use hypercolor_types::controls::{
    ActionConfirmationLevel, ControlAccess, ControlActionDescriptor, ControlAvailabilityState,
    ControlChange, ControlFieldDescriptor, ControlObjectField, ControlSurfaceDocument,
    ControlValue as DynamicControlValue, ControlValueMap, ControlValueType,
};
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::app::WsContext;
use crate::icons::*;
use crate::toasts;

#[component]
pub fn DeviceControlSurfaces(#[prop(into)] device_id: Signal<String>) -> impl IntoView {
    let ws_ctx = expect_context::<WsContext>();
    let surfaces_resource = LocalResource::new(move || {
        let id = device_id.get();
        async move { api::fetch_device_control_surfaces(&id, true).await }
    });

    Effect::new(move |_| {
        let Some(event) = ws_ctx.last_control_surface_event.get() else {
            return;
        };
        let current_device_id = device_id.get_untracked();
        if control_surface_event_matches_device(&event.surface_id, &current_device_id) {
            surfaces_resource.refetch();
        }
    });

    view! {
        <div class="rounded-xl bg-surface-raised border border-edge-subtle/60 overflow-hidden">
            <div class="px-4 py-3 border-b border-edge-subtle/60 flex items-center justify-between">
                <div class="flex items-center gap-2">
                    <Icon icon=LuSlidersHorizontal width="13px" height="13px" style="color: rgba(128, 255, 234, 0.7)" />
                    <h3 class="text-[11px] font-medium text-fg-secondary">"Controls"</h3>
                </div>
                <button
                    class="w-6 h-6 rounded-md flex items-center justify-center text-fg-tertiary/55 hover:text-fg-secondary hover:bg-surface-hover/40 transition-colors"
                    title="Refresh controls"
                    on:click=move |_| surfaces_resource.refetch()
                >
                    <Icon icon=LuRefreshCw width="12px" height="12px" />
                </button>
            </div>
            <div class="px-4 py-3">
                <Suspense fallback=move || view! {
                    <div class="h-16 rounded-lg bg-surface-overlay/20 animate-pulse" />
                }>
                    {move || match surfaces_resource.get() {
                        Some(Ok(surfaces)) if surfaces.is_empty() => view! {
                            <p class="text-[10px] text-fg-tertiary/50">"No dynamic controls exposed."</p>
                        }.into_any(),
                        Some(Ok(surfaces)) => view! {
                            <div class="space-y-3">
                                {surfaces.into_iter().map(|surface| {
                                    render_surface(surface, surfaces_resource)
                                }).collect_view()}
                            </div>
                        }.into_any(),
                        Some(Err(error)) => view! {
                            <div class="rounded-lg border border-edge-subtle/70 bg-surface-overlay/20 px-3 py-2">
                                <div class="text-[10px] text-fg-tertiary/60">{error}</div>
                            </div>
                        }.into_any(),
                        None => view! {
                            <div class="h-16 rounded-lg bg-surface-overlay/20 animate-pulse" />
                        }.into_any(),
                    }}
                </Suspense>
            </div>
        </div>
    }
}

fn render_surface(
    surface: ControlSurfaceDocument,
    surfaces_resource: LocalResource<Result<Vec<ControlSurfaceDocument>, String>>,
) -> impl IntoView {
    let field_count = surface.fields.len();
    let action_count = surface.actions.len();
    let title = surface_title(&surface);
    let subtitle = format!(
        "{field_count} fields · {action_count} actions · rev {}",
        surface.revision
    );
    let fields = surface.fields.clone();
    let actions = surface.actions.clone();

    view! {
        <section class="rounded-lg border border-edge-subtle/45 bg-surface-overlay/20 overflow-hidden">
            <div class="px-3 py-2 border-b border-edge-subtle/35">
                <div class="flex items-center justify-between gap-2">
                    <div class="min-w-0">
                        <div class="text-[11px] font-medium text-fg-secondary truncate">{title}</div>
                        <div class="text-[9px] font-mono text-fg-tertiary/45">{subtitle}</div>
                    </div>
                </div>
            </div>
            <div class="divide-y divide-edge-subtle/30">
                {fields.into_iter().map(|field| {
                    render_field(surface.clone(), field, surfaces_resource)
                }).collect_view()}
                {actions.into_iter().map(|action| {
                    render_action(surface.clone(), action, surfaces_resource)
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
    let availability = surface
        .availability
        .get(&field_id)
        .map(|availability| availability.state)
        .unwrap_or(ControlAvailabilityState::Available);
    let editable = field.access != ControlAccess::ReadOnly
        && availability == ControlAvailabilityState::Available;
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
        <div class="px-3 py-2.5 flex items-center gap-3">
            <div class="min-w-0 flex-1">
                <div class="text-[11px] text-fg-secondary font-medium truncate">{field_label}</div>
                {description.map(|text| view! {
                    <div class="text-[9px] text-fg-tertiary/45 leading-snug mt-0.5">{text}</div>
                })}
            </div>
            <div class="shrink-0 min-w-[120px] flex justify-end">{value_view}</div>
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
    match &field.value_type {
        ControlValueType::Bool => render_bool_editor(
            surface_id,
            revision,
            field.id.clone(),
            matches!(current_value, Some(DynamicControlValue::Bool(true))),
            editable,
            surfaces_resource,
        )
        .into_any(),
        ControlValueType::Integer { min, max, step } => render_number_editor(
            NumberEditorKind::Integer,
            surface_id,
            revision,
            field.id.clone(),
            number_text(current_value.as_ref()),
            Bounds {
                min: min.map(|v| v.to_string()),
                max: max.map(|v| v.to_string()),
                step: step.map_or_else(|| "1".to_string(), |v| v.to_string()),
            },
            editable,
            surfaces_resource,
        )
        .into_any(),
        ControlValueType::Float { min, max, step } => render_number_editor(
            NumberEditorKind::Float,
            surface_id,
            revision,
            field.id.clone(),
            number_text(current_value.as_ref()),
            Bounds {
                min: min.map(|v| v.to_string()),
                max: max.map(|v| v.to_string()),
                step: step.map_or_else(|| "0.01".to_string(), |v| v.to_string()),
            },
            editable,
            surfaces_resource,
        )
        .into_any(),
        ControlValueType::DurationMs { min, max, step } => render_number_editor(
            NumberEditorKind::DurationMs,
            surface_id,
            revision,
            field.id.clone(),
            number_text(current_value.as_ref()),
            Bounds {
                min: min.map(|v| v.to_string()),
                max: max.map(|v| v.to_string()),
                step: step.map_or_else(|| "100".to_string(), |v| v.to_string()),
            },
            editable,
            surfaces_resource,
        )
        .into_any(),
        ControlValueType::Enum { options } => render_enum_editor(
            surface_id,
            revision,
            field.id.clone(),
            enum_text(current_value.as_ref()),
            options
                .iter()
                .map(|option| (option.value.clone(), option.label.clone()))
                .collect(),
            editable,
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
            editable,
            surfaces_resource,
        )
        .into_any(),
        _ => view! {
            <span class="text-[10px] font-mono text-fg-tertiary/50">{value_text(current_value.as_ref())}</span>
        }
        .into_any(),
    }
}

fn render_bool_editor(
    surface_id: String,
    revision: u64,
    field_id: String,
    checked: bool,
    editable: bool,
    surfaces_resource: LocalResource<Result<Vec<ControlSurfaceDocument>, String>>,
) -> impl IntoView {
    view! {
        <input
            type="checkbox"
            class="w-4 h-4 rounded border-edge-subtle accent-cyan-300"
            prop:checked=checked
            disabled=!editable
            on:change=move |ev| {
                let event = Change::from_event(ev);
                if let Some(next) = event.checked() {
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

struct Bounds {
    min: Option<String>,
    max: Option<String>,
    step: String,
}

#[derive(Clone, Copy)]
enum NumberEditorKind {
    Integer,
    Float,
    DurationMs,
}

fn render_number_editor(
    kind: NumberEditorKind,
    surface_id: String,
    revision: u64,
    field_id: String,
    value: String,
    bounds: Bounds,
    editable: bool,
    surfaces_resource: LocalResource<Result<Vec<ControlSurfaceDocument>, String>>,
) -> impl IntoView {
    view! {
        <input
            type="number"
            class="w-28 bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1 text-[10px] font-mono text-fg-secondary
                   focus:outline-none focus:border-accent-muted disabled:opacity-50"
            prop:value=value
            min=bounds.min
            max=bounds.max
            step=bounds.step
            disabled=!editable
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

fn render_text_editor(
    kind: TextEditorKind,
    surface_id: String,
    revision: u64,
    field_id: String,
    value: String,
    editable: bool,
    surfaces_resource: LocalResource<Result<Vec<ControlSurfaceDocument>, String>>,
) -> impl IntoView {
    view! {
        <input
            type=if matches!(kind, TextEditorKind::Secret) { "password" } else { "text" }
            class="w-36 bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1 text-[10px] font-mono text-fg-secondary
                   focus:outline-none focus:border-accent-muted disabled:opacity-50"
            prop:value=value
            disabled=!editable
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

fn render_enum_editor(
    surface_id: String,
    revision: u64,
    field_id: String,
    value: String,
    options: Vec<(String, String)>,
    editable: bool,
    surfaces_resource: LocalResource<Result<Vec<ControlSurfaceDocument>, String>>,
) -> impl IntoView {
    view! {
        <select
            class="w-36 bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1 text-[10px] text-fg-secondary
                   focus:outline-none focus:border-accent-muted disabled:opacity-50"
            prop:value=value
            disabled=!editable
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
    let enabled = state == ControlAvailabilityState::Available;
    let label = action.label.clone();
    let description = action.description.clone();
    let confirmation = action.confirmation.clone();
    let confirmation_level = confirmation.as_ref().map(|confirmation| confirmation.level);
    let confirmation_message = confirmation
        .as_ref()
        .map(|confirmation| confirmation.message.clone());
    let confirmation_message_for_click = confirmation_message.clone();
    let input_fields = action.input_fields.clone();
    let (input_values, set_input_values) = signal(default_action_input_values(&input_fields));
    let (confirmation_armed, set_confirmation_armed) = signal(false);
    let button_label = Memo::new(move |_| {
        if confirmation.is_some() && confirmation_armed.get() {
            "Confirm".to_string()
        } else {
            "Run".to_string()
        }
    });

    view! {
        <div class="px-3 py-2.5 space-y-2">
            <div class="flex items-start gap-3">
                <div class="min-w-0 flex-1">
                    <div class="text-[11px] text-fg-secondary font-medium truncate">{label}</div>
                    {description.map(|text| view! {
                        <div class="text-[9px] text-fg-tertiary/45 leading-snug mt-0.5">{text}</div>
                    })}
                    <div class="text-[9px] text-fg-tertiary/45 font-mono mt-0.5">{format!("{state:?}")}</div>
                </div>
                <button
                    class=move || action_button_class(enabled, confirmation_level, confirmation_armed.get())
                    disabled=!enabled
                    on:click=move |_| {
                        let surface_id = surface.surface_id.clone();
                        let action_id = action_id.clone();
                        if let Some(message) = confirmation_message_for_click.clone()
                            && !confirmation_armed.get_untracked()
                        {
                            set_confirmation_armed.set(true);
                            toasts::toast_info(&message);
                            return;
                        }
                        let input = input_values.get_untracked();
                        leptos::task::spawn_local(async move {
                            match api::invoke_control_action(&surface_id, &action_id, input).await {
                                Ok(_) => {
                                    toasts::toast_success("Action sent");
                                    set_confirmation_armed.set(false);
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
                    {move || button_label.get()}
                </button>
            </div>
            {confirmation_message.map(|message| view! {
                <div class=confirmation_class(confirmation_level)>
                    <Icon icon=LuTriangleAlert width="10px" height="10px" />
                    <span>{message}</span>
                </div>
            })}
            {(!input_fields.is_empty()).then(|| view! {
                <div class="grid gap-2">
                    {input_fields.into_iter().map(|field| {
                        render_action_input(field, input_values, set_input_values, enabled)
                    }).collect_view()}
                </div>
            })}
        </div>
    }
}

fn render_action_input(
    field: ControlObjectField,
    input_values: ReadSignal<ControlValueMap>,
    set_input_values: WriteSignal<ControlValueMap>,
    enabled: bool,
) -> AnyView {
    let label = if field.required {
        format!("{} *", field.label)
    } else {
        field.label.clone()
    };
    let field_id = field.id.clone();
    let editor = render_action_input_editor(field, input_values, set_input_values, enabled);

    view! {
        <label class="flex items-center gap-2">
            <span class="min-w-[84px] max-w-[120px] truncate text-[9px] text-fg-tertiary/60">{label}</span>
            <div class="flex-1 min-w-0">{editor}</div>
            <span class="sr-only">{field_id}</span>
        </label>
    }
    .into_any()
}

fn render_action_input_editor(
    field: ControlObjectField,
    input_values: ReadSignal<ControlValueMap>,
    set_input_values: WriteSignal<ControlValueMap>,
    enabled: bool,
) -> AnyView {
    let field_id = field.id.clone();
    match field.value_type.clone() {
        ControlValueType::Bool => {
            let value_field = field_id.clone();
            let change_field = field_id.clone();
            view! {
                <input
                    type="checkbox"
                    class="w-4 h-4 rounded border-edge-subtle accent-cyan-300"
                    prop:checked=move || {
                        let values = input_values.get();
                        matches!(values.get(&value_field), Some(DynamicControlValue::Bool(true)))
                    }
                    disabled=!enabled
                    on:change=move |ev| {
                        if let Some(next) = Change::from_event(ev).checked() {
                            set_action_input_value(
                                set_input_values,
                                change_field.clone(),
                                DynamicControlValue::Bool(next),
                            );
                        }
                    }
                />
            }
            .into_any()
        }
        ControlValueType::Integer { .. } => render_action_number_input(
            NumberEditorKind::Integer,
            field_id,
            input_values,
            set_input_values,
            enabled,
        )
        .into_any(),
        ControlValueType::Float { .. } => render_action_number_input(
            NumberEditorKind::Float,
            field_id,
            input_values,
            set_input_values,
            enabled,
        )
        .into_any(),
        ControlValueType::DurationMs { .. } => render_action_number_input(
            NumberEditorKind::DurationMs,
            field_id,
            input_values,
            set_input_values,
            enabled,
        )
        .into_any(),
        ControlValueType::Enum { options } => {
            let value_field = field_id.clone();
            let change_field = field_id.clone();
            view! {
                <select
                    class="w-full bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1 text-[10px] text-fg-secondary
                           focus:outline-none focus:border-accent-muted disabled:opacity-50"
                    prop:value=move || {
                        let values = input_values.get();
                        enum_text(values.get(&value_field))
                    }
                    disabled=!enabled
                    on:change=move |ev| {
                        if let Some(raw) = Change::from_event(ev).value_string() {
                            set_action_input_value(
                                set_input_values,
                                change_field.clone(),
                                DynamicControlValue::Enum(raw),
                            );
                        }
                    }
                >
                    {options.into_iter().map(|option| view! {
                        <option value=option.value>{option.label}</option>
                    }).collect_view()}
                </select>
            }
            .into_any()
        }
        ControlValueType::String { .. }
        | ControlValueType::Secret
        | ControlValueType::IpAddress
        | ControlValueType::MacAddress
        | ControlValueType::ColorRgb
        | ControlValueType::ColorRgba => render_action_text_input(
            text_editor_kind(&field.value_type),
            field_id,
            input_values,
            set_input_values,
            enabled,
        )
        .into_any(),
        _ => view! {
            <span class="text-[9px] text-fg-tertiary/45">"Unsupported input"</span>
        }
        .into_any(),
    }
}

fn render_action_number_input(
    kind: NumberEditorKind,
    field_id: String,
    input_values: ReadSignal<ControlValueMap>,
    set_input_values: WriteSignal<ControlValueMap>,
    enabled: bool,
) -> impl IntoView {
    let value_field = field_id.clone();
    let change_field = field_id.clone();
    view! {
        <input
            type="number"
            class="w-full bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1 text-[10px] font-mono text-fg-secondary
                   focus:outline-none focus:border-accent-muted disabled:opacity-50"
            prop:value=move || {
                let values = input_values.get();
                number_text(values.get(&value_field))
            }
            disabled=!enabled
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
                set_action_input_value(set_input_values, change_field.clone(), parsed);
            }
        />
    }
}

fn render_action_text_input(
    kind: TextEditorKind,
    field_id: String,
    input_values: ReadSignal<ControlValueMap>,
    set_input_values: WriteSignal<ControlValueMap>,
    enabled: bool,
) -> impl IntoView {
    let value_field = field_id.clone();
    let change_field = field_id.clone();
    view! {
        <input
            type=if matches!(kind, TextEditorKind::Secret) { "password" } else { "text" }
            class="w-full bg-surface-sunken border border-edge-subtle rounded-md px-2 py-1 text-[10px] font-mono text-fg-secondary
                   focus:outline-none focus:border-accent-muted disabled:opacity-50"
            prop:value=move || {
                let values = input_values.get();
                value_text(values.get(&value_field))
            }
            disabled=!enabled
            on:change=move |ev| {
                if let Some(raw) = Change::from_event(ev).value_string() {
                    set_action_input_value(
                        set_input_values,
                        change_field.clone(),
                        text_value(kind, raw),
                    );
                }
            }
        />
    }
}

fn set_action_input_value(
    set_input_values: WriteSignal<ControlValueMap>,
    field_id: String,
    value: DynamicControlValue,
) {
    set_input_values.update(|values| {
        values.insert(field_id, value);
    });
}

fn default_action_input_values(fields: &[ControlObjectField]) -> ControlValueMap {
    fields
        .iter()
        .filter_map(|field| {
            field
                .default_value
                .clone()
                .map(|value| (field.id.clone(), value))
        })
        .collect()
}

fn action_button_class(
    enabled: bool,
    confirmation_level: Option<ActionConfirmationLevel>,
    armed: bool,
) -> &'static str {
    if !enabled {
        return "px-2 py-1 rounded-md text-[10px] font-medium transition-all btn-press flex items-center gap-1 bg-surface-hover/45 text-fg-secondary disabled:opacity-45 disabled:pointer-events-none";
    }

    match (confirmation_level, armed) {
        (Some(ActionConfirmationLevel::Destructive), true) => {
            "px-2 py-1 rounded-md text-[10px] font-medium transition-all btn-press flex items-center gap-1 bg-red-500/15 text-red-300 hover:text-red-200"
        }
        (Some(ActionConfirmationLevel::HardwarePersistent), true)
        | (Some(ActionConfirmationLevel::Normal), true)
        | (Some(ActionConfirmationLevel::Destructive), false) => {
            "px-2 py-1 rounded-md text-[10px] font-medium transition-all btn-press flex items-center gap-1 bg-yellow-500/10 text-yellow-200 hover:text-yellow-100"
        }
        _ => {
            "px-2 py-1 rounded-md text-[10px] font-medium transition-all btn-press flex items-center gap-1 bg-surface-hover/45 text-fg-secondary hover:text-accent"
        }
    }
}

fn confirmation_class(level: Option<ActionConfirmationLevel>) -> &'static str {
    match level {
        Some(ActionConfirmationLevel::Destructive) => {
            "flex items-center gap-1.5 text-[9px] text-red-300/75"
        }
        Some(ActionConfirmationLevel::HardwarePersistent) => {
            "flex items-center gap-1.5 text-[9px] text-yellow-200/75"
        }
        _ => "flex items-center gap-1.5 text-[9px] text-fg-tertiary/55",
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
                toasts::toast_success("Control updated");
                surfaces_resource.refetch();
            }
            Err(error) => {
                toasts::toast_error(&format!("Control update failed: {error}"));
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
    for index in 0..out.len() {
        let start = index * 2;
        let end = start + 2;
        if end <= hex.len()
            && let Ok(byte) = u8::from_str_radix(&hex[start..end], 16)
        {
            out[index] = byte;
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
    match value {
        Some(DynamicControlValue::String(value))
        | Some(DynamicControlValue::SecretRef(value))
        | Some(DynamicControlValue::IpAddress(value))
        | Some(DynamicControlValue::MacAddress(value)) => value.clone(),
        Some(DynamicControlValue::ColorRgb(value)) => {
            format!("#{:02x}{:02x}{:02x}", value[0], value[1], value[2])
        }
        Some(DynamicControlValue::ColorRgba(value)) => {
            format!(
                "#{:02x}{:02x}{:02x}{:02x}",
                value[0], value[1], value[2], value[3]
            )
        }
        Some(DynamicControlValue::Bool(value)) => value.to_string(),
        Some(DynamicControlValue::Integer(_))
        | Some(DynamicControlValue::Float(_))
        | Some(DynamicControlValue::DurationMs(_)) => number_text(value),
        Some(DynamicControlValue::Enum(_)) => enum_text(value),
        Some(DynamicControlValue::Flags(values)) => values.join(", "),
        Some(DynamicControlValue::List(_)) => "list".to_string(),
        Some(DynamicControlValue::Object(_)) => "object".to_string(),
        Some(DynamicControlValue::Null) | None => String::new(),
    }
}

fn surface_title(surface: &ControlSurfaceDocument) -> String {
    match &surface.scope {
        hypercolor_types::controls::ControlSurfaceScope::Driver { driver_id } => {
            format!("Driver · {driver_id}")
        }
        hypercolor_types::controls::ControlSurfaceScope::Device {
            driver_id,
            device_id,
        } => {
            if surface.surface_id.starts_with("driver:") {
                format!("{driver_id} device controls")
            } else {
                format!("Device · {device_id}")
            }
        }
    }
}

fn control_surface_event_matches_device(surface_id: &str, device_id: &str) -> bool {
    surface_id == format!("device:{device_id}")
        || surface_id.ends_with(&format!(":device:{device_id}"))
        || !surface_id.contains(":device:")
}
