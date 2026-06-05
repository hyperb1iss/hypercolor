use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::display_utils::display_preview_shell_url;
use crate::icons::*;

use super::{FaceCompositionSection, FaceControlsSection};

#[component]
pub(super) fn DisplayRightPanel(
    selected_display: Memo<Option<api::DisplaySummary>>,
    display_face: ReadSignal<Option<Result<Option<api::DisplayFaceResponse>, String>>>,
    set_display_face: WriteSignal<Option<Result<Option<api::DisplayFaceResponse>, String>>>,
    set_face_refresh_tick: WriteSignal<u64>,
    face_assignment_pending: ReadSignal<bool>,
    on_choose_face: Callback<()>,
    on_clear_face: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto pr-0.5" style="overscroll-behavior: contain;">
            <FaceAssignmentCard
                selected_display=selected_display
                display_face=display_face
                face_assignment_pending=face_assignment_pending
                on_choose_face=on_choose_face
                on_clear_face=on_clear_face
            />
            <FaceCompositionSection
                selected_display=selected_display
                display_face=display_face
                set_display_face=set_display_face
                set_face_refresh_tick=set_face_refresh_tick
            />
            <FaceControlsSection
                selected_display=selected_display
                display_face=display_face
                set_display_face=set_display_face
                set_face_refresh_tick=set_face_refresh_tick
            />
        </div>
    }
}

#[component]
fn FaceAssignmentCard(
    selected_display: Memo<Option<api::DisplaySummary>>,
    display_face: ReadSignal<Option<Result<Option<api::DisplayFaceResponse>, String>>>,
    face_assignment_pending: ReadSignal<bool>,
    on_choose_face: Callback<()>,
    on_clear_face: Callback<()>,
) -> impl IntoView {
    let face_name = Signal::derive(move || match display_face.get() {
        None => "Loading...".to_owned(),
        Some(Ok(Some(face))) => face.effect.name,
        Some(Ok(None)) => "No face assigned".to_owned(),
        Some(Err(_)) => "Face unavailable".to_owned(),
    });
    let face_author = Signal::derive(move || match display_face.get() {
        Some(Ok(Some(face))) => Some(face.effect.author),
        _ => None,
    });
    let face_description = Signal::derive(move || match display_face.get() {
        Some(Ok(Some(face))) => face.effect.description,
        Some(Err(error)) => error,
        _ => "Choose a face to start rendering.".to_owned(),
    });
    let has_face = Signal::derive(move || matches!(display_face.get(), Some(Ok(Some(_)))));

    view! {
        <div class="rounded-xl border border-t-2 border-edge-subtle border-t-coral/25 bg-surface-raised/80 p-3 edge-glow">
            <div class="flex items-center gap-2 border-b border-edge-subtle/50 pb-2">
                <div class="flex h-6 w-6 items-center justify-center rounded-md bg-coral/10 text-coral/80">
                    <Icon icon=LuLayers width="13" height="13" />
                </div>
                <h3 class="text-[11px] font-semibold uppercase tracking-wide text-fg-secondary">
                    "Face"
                </h3>
                <div class="flex-1" />
                <Show when=move || selected_display.with(Option::is_some) fallback=|| ()>
                    {move || selected_display.get().map(|display| {
                        let href = display_preview_shell_url(&display.id);
                        view! {
                            <a
                                href=href
                                target="_blank"
                                rel="noopener"
                                class="rounded-md p-1 text-fg-tertiary transition hover:text-coral"
                                title="Open full-screen preview"
                            >
                                <Icon icon=LuExternalLink width="11" height="11" />
                            </a>
                        }
                    })}
                </Show>
            </div>
            <div class="flex flex-col gap-2 pt-2.5">
                <div class="flex items-start justify-between gap-2">
                    <div class="min-w-0">
                        <div class="truncate text-sm font-medium text-fg-primary">
                            {move || face_name.get()}
                        </div>
                        {move || face_author.get().map(|author| view! {
                            <div class="mt-0.5 text-[10px] uppercase tracking-wider text-fg-tertiary">
                                {"by "}{author}
                            </div>
                        })}
                    </div>
                </div>
                <p class="text-[11px] leading-relaxed text-fg-secondary">
                    {move || face_description.get()}
                </p>
                <div class="mt-1 flex items-center gap-2">
                    <button
                        type="button"
                        class="inline-flex flex-1 items-center justify-center gap-1.5 rounded-md border border-coral/40 bg-coral/12 px-3 py-1.5 text-[11px] font-medium uppercase tracking-wider text-coral transition hover:bg-coral/20 disabled:cursor-not-allowed disabled:opacity-50"
                        disabled=move || face_assignment_pending.get()
                        on:click=move |_| on_choose_face.run(())
                    >
                        <Icon icon=LuLayers width="12" height="12" />
                        {move || if has_face.get() { "Change face" } else { "Choose face" }}
                    </button>
                    <button
                        type="button"
                        class="inline-flex items-center justify-center gap-1.5 rounded-md border border-edge-subtle bg-surface-overlay px-3 py-1.5 text-[11px] uppercase tracking-wider text-fg-tertiary transition hover:border-status-error/35 hover:text-status-error disabled:cursor-not-allowed disabled:opacity-40"
                        disabled=move || face_assignment_pending.get() || !has_face.get()
                        on:click=move |_| on_clear_face.run(())
                        title="Clear face assignment"
                    >
                        <Icon icon=LuX width="12" height="12" />
                        "Clear"
                    </button>
                </div>
            </div>
        </div>
    }
}
