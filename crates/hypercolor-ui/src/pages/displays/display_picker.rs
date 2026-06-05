use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::display_utils::is_simulator_display;
use crate::icons::*;

use super::DisplaysResource;

#[component]
pub(super) fn DisplayPicker(
    displays: DisplaysResource,
    selected_id: ReadSignal<Option<String>>,
    set_selected_id: WriteSignal<Option<String>>,
    on_create_simulator: Callback<()>,
    on_manage_simulator: Callback<api::DisplaySummary>,
) -> impl IntoView {
    view! {
        <div class="flex min-h-0 flex-1 flex-col overflow-hidden rounded-xl border border-edge-subtle bg-surface-raised/80 edge-glow">
            <header class="flex items-center justify-between border-b border-edge-subtle/50 px-3 py-2">
                <div class="flex items-center gap-2 text-[11px] uppercase tracking-wider text-fg-secondary">
                    <div class="flex h-5 w-5 items-center justify-center rounded-md bg-surface-overlay text-fg-secondary">
                        <Icon icon=LuMonitor width="11" height="11" />
                    </div>
                    <span class="font-semibold">"Displays"</span>
                </div>
                <button
                    type="button"
                    class="rounded-sm p-1 text-fg-tertiary transition hover:text-accent-primary"
                    title="Refresh displays"
                    on:click=move |_| displays.refetch()
                >
                    <Icon icon=LuRefreshCw width="12" height="12" />
                </button>
            </header>
            <div class="min-h-0 flex-1 overflow-y-auto">
                <Suspense fallback=move || view! { <PickerPlaceholder message="Loading displays...".to_string() /> }>
                    {move || {
                        let snapshot = displays.get();
                        let Some(result) = snapshot.as_ref() else {
                            return view! {
                                <PickerPlaceholder message="Loading displays...".to_string() />
                            }
                            .into_any();
                        };
                        match result {
                            Err(error) => view! {
                                <PickerPlaceholder message=error.clone() />
                            }
                            .into_any(),
                            Ok(items) if items.is_empty() => view! {
                                <div class="flex flex-col gap-3">
                                    <PickerPlaceholder
                                        message="No LCD devices connected. Connect a Corsair iCUE LINK pump, an Ableton Push 2, or add a virtual display simulator.".to_string()
                                    />
                                    <button
                                        type="button"
                                        class="inline-flex items-center gap-2 self-start rounded-md border border-accent-primary/35 bg-accent-primary/10 px-3 py-1.5 text-[11px] font-medium uppercase tracking-wider text-accent-primary transition hover:bg-accent-primary/15"
                                        on:click=move |_| on_create_simulator.run(())
                                    >
                                        <Icon icon=LuPlus width="12" height="12" />
                                        "Create simulator"
                                    </button>
                                </div>
                            }
                            .into_any(),
                            Ok(items) => {
                                let rows = items
                                    .clone()
                                    .into_iter()
                                    .map(|display| {
                                        render_picker_row(
                                            display,
                                            selected_id,
                                            set_selected_id,
                                            on_manage_simulator,
                                        )
                                    })
                                    .collect_view();
                                view! { <ul class="divide-y divide-edge-subtle">{rows}</ul> }
                                    .into_any()
                            }
                        }
                    }}
                </Suspense>
            </div>
        </div>
    }
}

#[component]
fn PickerPlaceholder(#[prop(into)] message: String) -> impl IntoView {
    view! {
        <div class="px-3 py-6 text-xs leading-relaxed text-fg-tertiary">{message}</div>
    }
}

fn render_picker_row(
    display: api::DisplaySummary,
    selected_id: ReadSignal<Option<String>>,
    set_selected_id: WriteSignal<Option<String>>,
    on_manage_simulator: Callback<api::DisplaySummary>,
) -> impl IntoView {
    let id = display.id.clone();
    let id_for_click = id.clone();
    let id_for_active = id.clone();
    let is_active = Signal::derive(move || {
        selected_id.with(|current| current.as_deref() == Some(id_for_active.as_str()))
    });
    let is_simulator = is_simulator_display(&display);
    let shape_label = if display.circular {
        "Round"
    } else if display.width > display.height * 2 {
        "Wide"
    } else {
        "Rect"
    };
    let dimensions = format!("{}x{}", display.width, display.height);
    let display_for_manage = display.clone();
    let vendor = display.vendor;
    let name = display.name;

    view! {
        <li class="flex items-stretch">
            <button
                type="button"
                class="flex min-w-0 flex-1 flex-col items-start gap-1 px-3 py-2 text-left transition hover:bg-surface-overlay"
                class:bg-surface-overlay=move || is_active.get()
                class:border-l-2=move || is_active.get()
                class:border-accent-primary=move || is_active.get()
                on:click=move |_| set_selected_id.set(Some(id_for_click.clone()))
            >
                <span class="flex w-full items-center justify-between gap-2">
                    <span class="flex min-w-0 items-center gap-2">
                        <span class="truncate text-sm font-medium text-fg-primary">{name}</span>
                        {if is_simulator {
                            view! {
                                <span class="shrink-0 rounded-sm border border-accent-primary/25 bg-accent-primary/10 px-1.5 py-0.5 text-[10px] uppercase tracking-wider text-accent-primary">
                                    "Simulator"
                                </span>
                            }
                            .into_any()
                        } else {
                            ().into_any()
                        }}
                    </span>
                    <span class="shrink-0 rounded-sm bg-surface-overlay px-1.5 py-0.5 text-[10px] uppercase tracking-wider text-fg-tertiary">
                        {shape_label}
                    </span>
                </span>
                <span class="flex items-center gap-2 text-[11px] text-fg-tertiary">
                    <span>{vendor}</span>
                    <span class="text-edge-subtle">"·"</span>
                    <span>{dimensions}</span>
                </span>
            </button>
            {if is_simulator {
                view! {
                    <button
                        type="button"
                        class="mx-2 my-2 shrink-0 self-start rounded-md border border-edge-subtle bg-surface-overlay/60 p-2 text-fg-tertiary transition hover:border-accent-primary/40 hover:text-accent-primary"
                        title="Edit simulator"
                        on:click=move |_| on_manage_simulator.run(display_for_manage.clone())
                    >
                        <Icon icon=LuSettings2 width="13" height="13" />
                    </button>
                }
                .into_any()
            } else {
                ().into_any()
            }}
        </li>
    }
}
