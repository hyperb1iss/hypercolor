use std::collections::BTreeSet;

use leptos::prelude::*;

use crate::api;
use crate::app::WsContext;

/// Follow one display's live preview while the caller is mounted.
pub fn use_display_preview_subscription(
    ws: WsContext,
    selected_display_id: Signal<Option<String>>,
) {
    use_display_previews(
        ws,
        Signal::derive(move || selected_display_id.get().into_iter().collect()),
    );
}

/// Follow the live previews of `display_ids` while the caller is mounted.
pub fn use_display_previews(ws: WsContext, display_ids: Signal<BTreeSet<String>>) {
    Effect::new(move |previous: Option<BTreeSet<String>>| {
        let current = display_ids.get();
        if previous.as_ref() != Some(&current) {
            ws.set_display_preview_devices.set(current.clone());
        }
        current
    });
    on_cleanup(move || {
        ws.set_display_preview_devices.set(BTreeSet::new());
    });
}

/// The latest preview frame of one display. It moves only when that
/// display's own frame does, however many others are followed beside it.
#[must_use]
pub fn display_preview_frame(
    ws: WsContext,
    device_id: Signal<Option<String>>,
) -> Signal<Option<crate::ws::CanvasFrame>> {
    let stamp = Memo::new(move |_| {
        let device_id = device_id.get()?;
        ws.display_preview_frames.with(|frames| {
            frames
                .get(&device_id)
                .map(|frame| (frame.frame_number, frame.timestamp_ms))
        })
    });
    Signal::derive(move || {
        stamp.get()?;
        let device_id = device_id.get_untracked()?;
        ws.display_preview_frames
            .with_untracked(|frames| frames.get(&device_id).cloned())
    })
}

pub fn use_display_face_resource(
    selected_display_id: Signal<Option<String>>,
    refresh_tick: Signal<u64>,
) -> LocalResource<api::ApiResult<Option<api::DisplayFaceResponse>>> {
    api::daemon_resource(move || {
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
