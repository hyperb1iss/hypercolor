use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use crate::api;
use crate::app::WsContext;
use crate::display_utils::display_preview_target_from_search;
use crate::icons::{LuLayers, LuMonitor};

type DisplaysResource = LocalResource<Result<Vec<api::DisplaySummary>, String>>;

#[component]
pub fn DisplayPreviewPage() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let displays: DisplaysResource = LocalResource::new(api::fetch_displays);
    let requested_display_id = StoredValue::new(current_preview_target());
    let (display_face, set_display_face) =
        signal(None::<Result<Option<api::DisplayFaceResponse>, String>>);

    let selected_display = Memo::new(move |_| {
        let snapshot = displays.get();
        let items = snapshot.as_ref()?.as_ref().ok()?;
        if let Some(requested) = requested_display_id.get_value() {
            return items.iter().find(|display| display.id == requested).cloned();
        }
        items.first().cloned()
    });
    let requested_display_missing = Memo::new(move |_| {
        let Some(requested) = requested_display_id.get_value() else {
            return false;
        };
        let Some(Ok(items)) = displays.get() else {
            return false;
        };
        !items.iter().any(|display| display.id == requested)
    });

    Effect::new(move |_| {
        let device_id = selected_display.with(|display| display.as_ref().map(|item| item.id.clone()));
        ws.set_display_preview_device.set(device_id);
    });
    on_cleanup(move || {
        ws.set_display_preview_device.set(None);
    });

    Effect::new(move |_| {
        let Some(display) = selected_display.get() else {
            set_display_face.set(None);
            return;
        };
        let display_id = display.id.clone();
        let requested_id = display_id.clone();
        spawn_local(async move {
            let result = api::fetch_display_face(&requested_id).await;
            if selected_display
                .get_untracked()
                .as_ref()
                .is_some_and(|current| current.id == requested_id)
            {
                set_display_face.set(Some(result));
            }
        });
    });

    let (preview_blob_url, set_preview_blob_url) = signal(None::<String>);
    Effect::new(move |previous: Option<Option<String>>| {
        if let Some(Some(old_url)) = previous.as_ref() {
            let _ = web_sys::Url::revoke_object_url(old_url);
        }

        let Some(frame) = ws.display_preview_frame.get() else {
            set_preview_blob_url.set(None);
            return None;
        };
        if !matches!(
            frame.pixel_format(),
            crate::ws::messages::CanvasPixelFormat::Jpeg
        ) {
            set_preview_blob_url.set(None);
            return None;
        }

        let parts = js_sys::Array::new();
        parts.push(frame.pixels_js());
        let options = web_sys::BlobPropertyBag::new();
        options.set_type("image/jpeg");
        let blob = match web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options) {
            Ok(blob) => blob,
            Err(_) => {
                set_preview_blob_url.set(None);
                return None;
            }
        };
        let url = match web_sys::Url::create_object_url_with_blob(&blob) {
            Ok(url) => url,
            Err(_) => {
                set_preview_blob_url.set(None);
                return None;
            }
        };
        set_preview_blob_url.set(Some(url.clone()));
        Some(url)
    });

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

                        let src = preview_blob_url
                            .get()
                            .unwrap_or_else(|| api::display_preview_url(&display.id, None));
                        let aspect = format!("{} / {}", display.width, display.height);
                        let rounded_class = if display.circular {
                            "rounded-full"
                        } else {
                            "rounded-[2rem]"
                        };
                        let alt_text = format!("Full-screen preview of {}", display.name);
                        let frame_class = format!(
                            "relative max-h-[calc(100vh-3rem)] max-w-[calc(100vw-3rem)] overflow-hidden border border-white/10 bg-black shadow-[0_0_80px_rgba(0,0,0,0.65)] {rounded_class}"
                        );

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
                            <div
                                class=frame_class
                                style=move || format!("aspect-ratio: {aspect};")
                            >
                                <img
                                    class="h-full w-full object-cover"
                                    src=src
                                    alt=alt_text
                                    loading="eager"
                                    decoding="async"
                                    draggable="false"
                                />
                            </div>
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

fn current_preview_target() -> Option<String> {
    web_sys::window()
        .map(|window| window.location())
        .and_then(|location| location.search().ok())
        .and_then(|search| display_preview_target_from_search(&search))
}
