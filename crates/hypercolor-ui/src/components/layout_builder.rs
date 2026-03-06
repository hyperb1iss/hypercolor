//! Layout builder wrapper — toolbar + three-column layout editor.

use leptos::prelude::*;
use leptos_icons::Icon;
use wasm_bindgen::JsCast;

use crate::api;
use crate::app::DevicesContext;
use crate::icons::*;
use crate::toasts;
use crate::components::layout_canvas::LayoutCanvas;
use crate::components::layout_palette::LayoutPalette;
use crate::components::layout_zone_properties::LayoutZoneProperties;
use hypercolor_types::spatial::SpatialLayout;

/// Layout builder — wraps toolbar, palette, canvas viewport, and zone properties.
#[component]
pub fn LayoutBuilder() -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();

    let (selected_layout_id, set_selected_layout_id) = signal(None::<String>);
    let (layout, set_layout) = signal(None::<SpatialLayout>);
    let (selected_zone_id, set_selected_zone_id) = signal(None::<String>);
    let (is_dirty, set_is_dirty) = signal(false);
    let (creating, set_creating) = signal(false);
    let (new_layout_name, set_new_layout_name) = signal(String::new());
    let (initialized, set_initialized) = signal(false);
    let layout_signal = Signal::derive(move || layout.get());
    let zone_id_signal = Signal::derive(move || selected_zone_id.get());
    let has_layout = Signal::derive(move || layout.with(|current| current.is_some()));

    // Auto-select the active layout (or first available, or create a default) on mount
    Effect::new(move |_| {
        if initialized.get() {
            return;
        }
        // Wait for layouts resource to load
        let Some(Ok(layouts)) = ctx.layouts_resource.get() else {
            return;
        };
        set_initialized.set(true);

        if layouts.is_empty() {
            // No layouts exist — create a default one
            let layouts_resource = ctx.layouts_resource;
            let set_id = set_selected_layout_id;
            leptos::task::spawn_local(async move {
                let req = api::CreateLayoutRequest {
                    name: "Default Layout".to_string(),
                    description: None,
                    canvas_width: None,
                    canvas_height: None,
                };
                if let Ok(summary) = api::create_layout(&req).await {
                    layouts_resource.refetch();
                    set_id.set(Some(summary.id));
                }
            });
        } else {
            // Try to load the active layout first, otherwise pick the first one
            let set_id = set_selected_layout_id;
            let first_id = layouts[0].id.clone();
            leptos::task::spawn_local(async move {
                if let Ok(active) = api::fetch_active_layout().await {
                    set_id.set(Some(active.id));
                } else {
                    set_id.set(Some(first_id));
                }
            });
        }
    });

    // Load layout when selection changes
    Effect::new(move |_| {
        let id = selected_layout_id.get();
        if let Some(id) = id {
            let set_layout = set_layout;
            leptos::task::spawn_local(async move {
                if let Ok(l) = api::fetch_layout(&id).await {
                    set_layout.set(Some(l));
                }
            });
        } else {
            set_layout.set(None);
        }
        set_selected_zone_id.set(None);
        set_is_dirty.set(false);
    });

    // Save handler
    let save_layout = move || {
        let Some(l) = layout.get_untracked() else { return };
        let id = l.id.clone();
        let zones = l.zones.clone();
        let layouts_resource = ctx.layouts_resource;
        leptos::task::spawn_local(async move {
            let req = api::UpdateLayoutApiRequest {
                name: None,
                description: None,
                canvas_width: None,
                canvas_height: None,
                zones: Some(zones),
            };
            if api::update_layout(&id, &req).await.is_ok() {
                toasts::toast_success("Layout saved");
                set_is_dirty.set(false);
            } else {
                toasts::toast_error("Failed to save layout");
            }
            layouts_resource.refetch();
        });
    };

    // Apply handler
    let apply_layout = move || {
        let Some(l) = layout.get_untracked() else { return };
        let id = l.id.clone();
        let zones = l.zones.clone();
        let layouts_resource = ctx.layouts_resource;
        leptos::task::spawn_local(async move {
            let req = api::UpdateLayoutApiRequest {
                name: None,
                description: None,
                canvas_width: None,
                canvas_height: None,
                zones: Some(zones),
            };

            if api::update_layout(&id, &req).await.is_err() {
                toasts::toast_error("Failed to save layout before apply");
                return;
            }

            if api::apply_layout(&id).await.is_ok() {
                toasts::toast_success("Layout applied");
                set_is_dirty.set(false);
            } else {
                toasts::toast_error("Failed to apply layout");
            }
            layouts_resource.refetch();
        });
    };

    // Delete handler
    let delete_layout = move || {
        let Some(l) = layout.get() else { return };
        let id = l.id.clone();
        let layouts_resource = ctx.layouts_resource;
        set_selected_layout_id.set(None);
        set_layout.set(None);
        leptos::task::spawn_local(async move {
            if api::delete_layout(&id).await.is_ok() {
                toasts::toast_info("Layout deleted");
            }
            layouts_resource.refetch();
        });
    };

    // Create handler
    let create_layout = move || {
        let name = new_layout_name.get();
        if name.trim().is_empty() {
            return;
        }
        set_creating.set(false);
        set_new_layout_name.set(String::new());
        let layouts_resource = ctx.layouts_resource;
        let set_id = set_selected_layout_id;
        leptos::task::spawn_local(async move {
            let req = api::CreateLayoutRequest {
                name,
                description: None,
                canvas_width: None,
                canvas_height: None,
            };
            if let Ok(summary) = api::create_layout(&req).await {
                toasts::toast_success("Layout created");
                layouts_resource.refetch();
                set_id.set(Some(summary.id));
            }
        });
    };

    view! {
        <div class="flex flex-col flex-1 overflow-hidden">
            // Toolbar
            <div class="shrink-0 px-6 py-3 flex items-center gap-3 bg-layer-0 border-b border-white/[0.04]">
                // Layout selector
                <Suspense fallback=|| ()>
                    {move || {
                        ctx.layouts_resource.get().map(|result| {
                            let layouts = result.unwrap_or_default();
                            view! {
                                <select
                                    class="bg-layer-2 border border-white/[0.06] rounded-lg px-3 py-1.5 text-sm text-fg
                                           focus:outline-none focus:border-electric-purple/20 min-w-[180px]"
                                    on:change=move |ev| {
                                        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok());
                                        if let Some(el) = target {
                                            let val = el.value();
                                            if val.is_empty() {
                                                set_selected_layout_id.set(None);
                                            } else {
                                                set_selected_layout_id.set(Some(val));
                                            }
                                        }
                                    }
                                >
                                    <option value="" selected=move || selected_layout_id.get().is_none()>"Select layout..."</option>
                                    {layouts.into_iter().map(|l| {
                                        let lid = l.id.clone();
                                        let lid2 = l.id.clone();
                                        view! {
                                            <option
                                                value=lid
                                                selected=move || selected_layout_id.get().as_deref() == Some(&lid2)
                                            >
                                                {l.name} " (" {l.zone_count} " zones)"
                                            </option>
                                        }
                                    }).collect_view()}
                                </select>
                            }
                        })
                    }}
                </Suspense>

                // New layout button / form
                {move || if creating.get() {
                    view! {
                        <div class="flex items-center gap-2">
                            <input
                                type="text"
                                placeholder="Layout name"
                                class="bg-layer-2 border border-white/[0.06] rounded-lg px-3 py-1.5 text-sm text-fg
                                       placeholder-fg-dim focus:outline-none focus:border-electric-purple/20 w-40"
                                prop:value=move || new_layout_name.get()
                                on:input=move |ev| {
                                    let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                    if let Some(el) = target { set_new_layout_name.set(el.value()); }
                                }
                                on:keydown=move |ev| {
                                    if ev.key() == "Enter" { create_layout(); }
                                    if ev.key() == "Escape" { set_creating.set(false); }
                                }
                            />
                            <button
                                class="px-3 py-1.5 rounded-lg text-xs font-medium bg-success-green/[0.1] border border-success-green/20
                                       text-success-green hover:bg-success-green/[0.2] transition-all btn-press"
                                on:click=move |_| create_layout()
                            >"Create"</button>
                            <button
                                class="px-3 py-1.5 rounded-lg text-xs font-medium bg-white/[0.04] border border-white/[0.06]
                                       text-fg-dim hover:text-fg hover:bg-white/[0.08] transition-all btn-press"
                                on:click=move |_| set_creating.set(false)
                            >"Cancel"</button>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <button
                            class="px-3 py-1.5 rounded-lg text-xs font-medium bg-electric-purple/[0.08] border border-electric-purple/20
                                   text-electric-purple hover:bg-electric-purple/[0.15] transition-all btn-press"
                            on:click=move |_| set_creating.set(true)
                        >
                            <Icon icon=LuPlus width="14px" height="14px" />
                            " New Layout"
                        </button>
                    }.into_any()
                }}

                <div class="flex-1" />

                // Save / Apply / Delete buttons — only when a layout is loaded
                {move || layout.get().map(|_| {
                    let save_style = if is_dirty.get() {
                        "background: rgba(80, 250, 123, 0.1); border-color: rgba(80, 250, 123, 0.2); color: rgb(80, 250, 123)"
                    } else {
                        "background: rgba(255, 255, 255, 0.04); border-color: rgba(255, 255, 255, 0.06); color: rgba(161, 161, 170, 1)"
                    };
                    view! {
                        <div class="flex items-center gap-2">
                            <button
                                class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium border transition-all btn-press"
                                style=save_style
                                on:click=move |_| save_layout()
                            >
                                <Icon icon=LuSave width="14px" height="14px" />
                                {move || if is_dirty.get() { "Save *" } else { "Save" }}
                            </button>
                            <button
                                class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium bg-neon-cyan/[0.08] border border-neon-cyan/20
                                       text-neon-cyan hover:bg-neon-cyan/[0.15] transition-all btn-press"
                                on:click=move |_| apply_layout()
                            >
                                <Icon icon=LuPlay width="14px" height="14px" />
                                "Apply"
                            </button>
                            <button
                                class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium bg-error-red/[0.08] border border-error-red/20
                                       text-error-red hover:bg-error-red/[0.15] transition-all btn-press"
                                on:click=move |_| delete_layout()
                            >
                                <Icon icon=LuTrash2 width="14px" height="14px" />
                                "Delete"
                            </button>
                        </div>
                    }
                })}
            </div>

            // Three-column layout
            <Show
                when=move || has_layout.get()
                fallback=move || {
                    view! {
                        <div class="flex-1 flex items-center justify-center">
                            <div class="text-center space-y-2">
                                <div class="text-fg-dim text-sm">"Select or create a layout to begin"</div>
                                <div class="text-fg-dim/50 text-xs">"Drag devices onto the canvas to build your spatial mapping"</div>
                            </div>
                        </div>
                    }
                }
            >
                <div class="flex flex-1 overflow-hidden">
                    // Left palette
                    <div class="w-[200px] shrink-0 border-r border-white/[0.04] overflow-y-auto">
                        <LayoutPalette
                            layout=layout_signal
                            set_layout=set_layout
                            set_selected_zone_id=set_selected_zone_id
                            set_is_dirty=set_is_dirty
                        />
                    </div>

                    // Center canvas
                    <div class="flex-1 overflow-hidden relative">
                        <LayoutCanvas
                            layout=layout_signal
                            selected_zone_id=zone_id_signal
                            set_selected_zone_id=set_selected_zone_id
                            set_layout=set_layout
                            set_is_dirty=set_is_dirty
                        />
                    </div>

                    // Right properties
                    <div class="w-[280px] shrink-0 border-l border-white/[0.04] overflow-y-auto">
                        <LayoutZoneProperties
                            layout=layout_signal
                            selected_zone_id=zone_id_signal
                            set_layout=set_layout
                            set_selected_zone_id=set_selected_zone_id
                            set_is_dirty=set_is_dirty
                        />
                    </div>
                </div>
            </Show>
        </div>
    }
}
