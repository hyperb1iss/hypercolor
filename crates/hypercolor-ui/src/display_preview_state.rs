use leptos::prelude::*;

use crate::api;
use crate::app::WsContext;

pub fn use_display_preview_subscription(
    ws: WsContext,
    selected_display_id: Signal<Option<String>>,
) {
    Effect::new(move |_| {
        ws.set_display_preview_device.set(selected_display_id.get());
    });
    on_cleanup(move || {
        ws.set_display_preview_device.set(None);
    });
}

pub fn use_display_face_resource(
    selected_display_id: Signal<Option<String>>,
    refresh_tick: Signal<u64>,
) -> LocalResource<Result<Option<api::DisplayFaceResponse>, String>> {
    LocalResource::new(move || {
        let selected_display_id = selected_display_id.get();
        let _refresh_tick = refresh_tick.get();

        async move {
            match selected_display_id {
                Some(display_id) => api::fetch_display_face(&display_id).await,
                None => Ok(None),
            }
        }
    })
}
