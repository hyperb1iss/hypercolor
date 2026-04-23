use leptos::prelude::*;
use leptos_icons::Icon;
use leptos_router::hooks::use_query_map;

use crate::api;
use crate::app::{DisplaysContext, WsContext};
use crate::components::display_preview_surface::DisplayPreviewSurface;
use crate::display_preview_state::{use_display_face_resource, use_display_preview_subscription};
use crate::icons::{LuLayers, LuMonitor};
use hypercolor_types::scene::RenderGroupRole;

#[component]
pub fn DisplayPreviewPage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let displays = expect_context::<DisplaysContext>().displays_resource;
    let query = use_query_map();
    let requested_display_id = Memo::new(move |_| query.with(|map| map.get("display")));
    let (face_refresh_tick, set_face_refresh_tick) = signal(0_u64);

    let selected_display = Memo::new(move |_| {
        let snapshot = displays.get();
        let items = snapshot.as_ref()?.as_ref().ok()?;
        if let Some(requested) = requested_display_id.get() {
            return items
                .iter()
                .find(|display| display.id == requested)
                .cloned();
        }
        items.first().cloned()
    });
    let requested_display_missing = Memo::new(move |_| {
        let Some(requested) = requested_display_id.get() else {
            return false;
        };
        let Some(Ok(items)) = displays.get() else {
            return false;
        };
        !items.iter().any(|display| display.id == requested)
    });
    let selected_display_id = Signal::derive(move || {
        selected_display.with(|display| display.as_ref().map(|item| item.id.clone()))
    });
    use_display_preview_subscription(ws, selected_display_id);
    let preview_frame = Signal::derive(move || ws.display_preview_frame.get());
    let display_face = use_display_face_resource(
        selected_display_id,
        Signal::derive(move || face_refresh_tick.get()),
    );

    Effect::new(
        move |previous_scene_event: Option<Option<crate::ws::SceneEventHint>>| {
            let current_scene_event = ws.last_scene_event.get();
            if previous_scene_event.as_ref() == Some(&current_scene_event) {
                return current_scene_event;
            }

            if current_scene_event.as_ref().is_some_and(|scene_event| {
                scene_event.event_type == "active_scene_changed"
                    || scene_event.render_group_role == Some(RenderGroupRole::Display)
            }) {
                set_face_refresh_tick.update(|tick| *tick = tick.wrapping_add(1));
            }

            current_scene_event
        },
    );

    let face_name = Signal::derive(move || match display_face.get() {
        Some(Ok(Some(face))) => Some(face.effect.name),
        _ => None,
    });

    view! {
        <div class="fixed inset-0 flex items-center justify-center overflow-hidden bg-black text-fg-primary">
            {move || {
                if requested_display_missing.get() {
                    return view! {
                        <PreviewShellMessage
                            icon=LuMonitor
                            title="Display not found"
                            body="The requested display preview target is unavailable."
                        />
                    }
                    .into_any();
                }

                match displays.get() {
                    None => view! {
                        <PreviewShellMessage
                            icon=LuMonitor
                            title="Loading preview"
                            body="Waiting for display metadata."
                        />
                    }
                    .into_any(),
                    Some(Err(error)) => view! {
                        <PreviewShellMessage
                            icon=LuMonitor
                            title="Preview unavailable"
                            body=error
                        />
                    }
                    .into_any(),
                    Some(Ok(_)) => {
                        let Some(display) = selected_display.get() else {
                            return view! {
                                <PreviewShellMessage
                                    icon=LuMonitor
                                    title="No displays"
                                    body="Connect an LCD device or create a simulator to use the preview shell."
                                />
                            }
                            .into_any();
                        };

                        let aspect = format!("{} / {}", display.width, display.height);
                        let container_class = if display.circular {
                            "relative max-h-[calc(100vh-3rem)] max-w-[calc(100vw-3rem)] overflow-hidden rounded-full border border-white/10 bg-black shadow-[0_0_80px_rgba(0,0,0,0.65)]"
                        } else {
                            "relative max-h-[calc(100vh-3rem)] max-w-[calc(100vw-3rem)] overflow-hidden rounded-[2rem] border border-white/10 bg-black shadow-[0_0_80px_rgba(0,0,0,0.65)]"
                        };
                        let alt_text = format!("Full-screen preview of {}", display.name);
                        let fallback_src =
                            api::display_preview_url(&display.id, Some(face_refresh_tick.get()));

                        view! {
                            <div class="absolute inset-x-0 top-0 z-10 flex items-start justify-between gap-3 p-4">
                                <div class="inline-flex min-w-0 items-center gap-2 rounded-full border border-white/12 bg-black/55 px-3 py-1.5 text-[11px] uppercase tracking-[0.18em] text-white/78 backdrop-blur-md">
                                    <Icon icon=LuMonitor width="12" height="12" />
                                    <span class="truncate">{display.name.clone()}</span>
                                    <span class="text-white/35">"·"</span>
                                    <span>{format!("{}x{}", display.width, display.height)}</span>
                                </div>
                                {move || face_name.get().map(|name| view! {
                                    <div class="inline-flex min-w-0 items-center gap-2 rounded-full border border-coral/30 bg-black/55 px-3 py-1.5 text-[11px] uppercase tracking-[0.18em] text-coral backdrop-blur-md">
                                        <Icon icon=LuLayers width="12" height="12" />
                                        <span class="truncate">{name}</span>
                                    </div>
                                })}
                            </div>
                            <DisplayPreviewSurface
                                frame=preview_frame
                                fallback_src=fallback_src
                                aspect_ratio=aspect
                                aria_label=alt_text
                                container_class=container_class
                            />
                        }
                        .into_any()
                    }
                }
            }}
        </div>
    }
}

#[component]
fn PreviewShellMessage(
    icon: icondata_core::Icon,
    title: &'static str,
    #[prop(into)] body: String,
) -> impl IntoView {
    view! {
        <div class="flex flex-col items-center gap-3 rounded-[1.5rem] border border-white/10 bg-white/[0.04] px-8 py-7 text-center shadow-[0_0_80px_rgba(0,0,0,0.4)] backdrop-blur-md">
            <div class="flex h-12 w-12 items-center justify-center rounded-full border border-white/12 bg-white/[0.06] text-white/82">
                <Icon icon=icon width="20" height="20" />
            </div>
            <div class="space-y-1">
                <div class="text-sm font-semibold uppercase tracking-[0.18em] text-white/92">
                    {title}
                </div>
                <p class="max-w-md text-sm leading-6 text-white/62">{body}</p>
            </div>
        </div>
    }
}
