use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;
use leptos_use::use_debounce_fn;

use crate::api;
use crate::app::EffectsContext;
use crate::face_blend::{FACE_BLEND_OPTIONS, FACE_BLEND_PRESETS, FaceBlendPreset, face_blend_option};
use crate::icons::*;
use crate::toasts;
use hypercolor_leptos_ext::events::Input;
use hypercolor_types::scene::DisplayFaceBlendMode;

use super::snapshot_scene_lock_message;

fn sync_face_composition_from_server(
    display_face: ReadSignal<Option<Result<Option<api::DisplayFaceResponse>, String>>>,
    set_local_blend_mode: WriteSignal<DisplayFaceBlendMode>,
    set_local_opacity: WriteSignal<f32>,
) {
    let target = display_face
        .get_untracked()
        .and_then(Result::ok)
        .flatten()
        .and_then(|face| face.group.display_target);
    if let Some(target) = target {
        set_local_blend_mode.set(target.blend_mode);
        set_local_opacity.set(target.opacity.clamp(0.0, 1.0));
    } else {
        set_local_blend_mode.set(DisplayFaceBlendMode::Alpha);
        set_local_opacity.set(1.0);
    }
}

#[component]
pub(super) fn FaceCompositionSection(
    selected_display: Memo<Option<api::DisplaySummary>>,
    display_face: ReadSignal<Option<Result<Option<api::DisplayFaceResponse>, String>>>,
    set_display_face: WriteSignal<Option<Result<Option<api::DisplayFaceResponse>, String>>>,
    set_face_refresh_tick: WriteSignal<u64>,
) -> impl IntoView {
    let effects_ctx = use_context::<EffectsContext>();
    let (local_blend_mode, set_local_blend_mode) = signal(DisplayFaceBlendMode::Alpha);
    let (local_opacity, set_local_opacity) = signal(1.0_f32);
    let has_face = Signal::derive(move || matches!(display_face.get(), Some(Ok(Some(_)))));
    let selected_blend_option = Memo::new(move |_| face_blend_option(local_blend_mode.get()));

    Effect::new(move |_| {
        sync_face_composition_from_server(display_face, set_local_blend_mode, set_local_opacity);
    });

    let commit_composition = Callback::new(
        move |(blend_mode, opacity): (Option<DisplayFaceBlendMode>, Option<f32>)| {
            if let Some(message) =
                snapshot_scene_lock_message(effects_ctx, "changing display face composition")
            {
                sync_face_composition_from_server(
                    display_face,
                    set_local_blend_mode,
                    set_local_opacity,
                );
                toasts::toast_error(&message);
                return;
            }
            let Some(display) = selected_display.get_untracked() else {
                return;
            };
            let display_id = display.id.clone();
            spawn_local(async move {
                match api::update_display_face_composition(&display_id, blend_mode, opacity).await {
                    Ok(face) => {
                        set_display_face.set(Some(Ok(Some(face))));
                        set_face_refresh_tick.update(|value| *value = value.wrapping_add(1));
                    }
                    Err(error) => {
                        sync_face_composition_from_server(
                            display_face,
                            set_local_blend_mode,
                            set_local_opacity,
                        );
                        toasts::toast_error(&format!("Face composition update failed: {error}"));
                    }
                }
            });
        },
    );

    let commit_opacity = use_debounce_fn(
        move || {
            if !local_blend_mode.get_untracked().blends_with_effect() {
                return;
            }
            commit_composition.run((None, Some(local_opacity.get_untracked())));
        },
        90.0,
    );

    let set_mode = Callback::new(move |mode: DisplayFaceBlendMode| {
        set_local_blend_mode.set(mode);
        let opacity = if mode.blends_with_effect() {
            local_opacity.get_untracked()
        } else {
            1.0
        };
        commit_composition.run((Some(mode), Some(opacity)));
    });

    let on_opacity_input = Callback::new(move |event| {
        let Some(raw) = Input::from_event(event).value::<f32>() else {
            return;
        };
        set_local_opacity.set((raw / 100.0).clamp(0.0, 1.0));
        commit_opacity();
    });

    let apply_preset = Callback::new(move |preset: FaceBlendPreset| {
        set_local_blend_mode.set(preset.mode);
        set_local_opacity.set(preset.opacity);
        commit_composition.run((Some(preset.mode), Some(preset.opacity)));
    });

    view! {
        <Show when=move || has_face.get() fallback=|| ()>
            <div
                class="rounded-xl bg-surface-raised/80 border border-edge-subtle p-3 edge-glow"
                style="border-top: 2px solid rgba(255, 106, 193, 0.2)"
            >
                <div class="mb-3 flex items-center gap-2 border-b border-edge-subtle/50 pb-2">
                    <div
                        class="flex h-6 w-6 items-center justify-center rounded-md"
                        style="background: rgba(255, 106, 193, 0.1); box-shadow: 0 0 8px rgba(255, 106, 193, 0.08)"
                    >
                        <span style="color: rgba(255, 106, 193, 0.7)">
                            <Icon icon=LuSlidersHorizontal width="13" height="13" />
                        </span>
                    </div>
                    <h3 class="text-[11px] font-semibold uppercase tracking-wide text-fg-secondary">
                        "Composition"
                    </h3>
                    <div class="flex-1" />
                    <span class="text-[10px] text-fg-tertiary">
                        {move || selected_blend_option.get().label}
                    </span>
                </div>

                <FaceBlendModeSelect
                    local_blend_mode=local_blend_mode
                    set_mode=set_mode
                />

                <p class="mt-2 px-1 text-[10px] leading-relaxed text-fg-tertiary/80">
                    {move || selected_blend_option.get().blurb}
                </p>

                <Show when=move || local_blend_mode.get().blends_with_effect() fallback=|| ()>
                    <div class="mt-3 flex items-center gap-2.5 rounded-lg px-3 py-2 hover:bg-surface-hover/20 transition-colors duration-200">
                        <Icon
                            icon=LuGauge
                            width="15px"
                            height="15px"
                            style="color: rgba(255, 106, 193, 0.6); flex-shrink: 0"
                        />
                        <label class="text-xs text-fg-secondary font-medium shrink-0 min-w-[80px] max-w-[120px] truncate">
                            "Blend Amount"
                        </label>
                        <div class="flex-1 min-w-0 flex items-center gap-2">
                            <input
                                type="range"
                                min="0"
                                max="100"
                                step="1"
                                class="flex-1 accent-coral"
                                prop:value=move || format!("{:.0}", local_opacity.get() * 100.0)
                                on:input=move |event| on_opacity_input.run(event)
                            />
                            <span class="text-[11px] font-mono text-fg-primary tabular-nums w-10 text-right">
                                {move || format!("{:.0}%", local_opacity.get() * 100.0)}
                            </span>
                        </div>
                    </div>
                </Show>

                <div class="mt-2 flex items-center gap-2.5 mb-1.5 px-1">
                    <div
                        style="background: linear-gradient(to right, transparent, rgba(255, 106, 193, 0.25), transparent)"
                        class="h-px flex-1"
                    />
                    <span
                        style="color: rgba(255, 106, 193, 0.5)"
                        class="text-[9px] font-mono uppercase tracking-[0.15em] shrink-0"
                    >
                        "Quick Looks"
                    </span>
                    <div
                        style="background: linear-gradient(to right, transparent, rgba(255, 106, 193, 0.25), transparent)"
                        class="h-px flex-1"
                    />
                </div>
                <div class="flex flex-wrap gap-1.5 px-1">
                    {FACE_BLEND_PRESETS
                        .iter()
                        .copied()
                        .map(|preset| {
                            view! {
                                <button
                                    type="button"
                                    class=move || {
                                        if local_blend_mode.get() == preset.mode
                                            && (local_opacity.get() - preset.opacity).abs() <= 0.01
                                        {
                                            "inline-flex items-center rounded-md border border-coral/50 bg-coral/12 px-2.5 py-1 text-[10px] font-medium text-coral transition"
                                        } else {
                                            "inline-flex items-center rounded-md border border-edge-subtle bg-surface-overlay/50 px-2.5 py-1 text-[10px] text-fg-secondary transition hover:border-coral/40 hover:text-fg-primary"
                                        }
                                    }
                                    on:click=move |_| apply_preset.run(preset)
                                >
                                    {preset.label}
                                </button>
                            }
                        })
                        .collect_view()}
                </div>
            </div>
        </Show>
    }
}

#[component]
fn FaceBlendModeSelect(
    local_blend_mode: ReadSignal<DisplayFaceBlendMode>,
    set_mode: Callback<DisplayFaceBlendMode>,
) -> impl IntoView {
    let (dropdown_open, set_dropdown_open) = signal(false);
    let trigger_ref = NodeRef::<leptos::html::Button>::new();
    let dropdown_class = "control-dropdown-face-blend".to_owned();
    let dropdown_wrapper_class = dropdown_class.clone();
    let dropdown_class_value = StoredValue::new(dropdown_class.clone());

    view! {
        <div
            class="flex items-center gap-2.5 rounded-lg px-3 py-2 hover:bg-surface-hover/20 transition-colors duration-200 group"
            class=("relative", move || dropdown_open.get())
            class=("z-[100]", move || dropdown_open.get())
        >
            <Icon
                icon=LuLayers
                width="15px"
                height="15px"
                style="color: rgba(255, 106, 193, 0.6); flex-shrink: 0"
            />
            <label class="text-xs text-fg-secondary font-medium shrink-0 min-w-[80px] max-w-[120px] truncate">
                "Blend Mode"
            </label>
            <div class=format!("relative flex-1 min-w-0 {dropdown_wrapper_class}")>
                <button
                    type="button"
                    node_ref=trigger_ref
                    class="w-full flex items-center gap-1.5 bg-surface-sunken border px-2.5 py-1.5
                           text-xs cursor-pointer select-silk-trigger"
                    class=("rounded-t-lg", move || dropdown_open.get())
                    class=("rounded-lg", move || !dropdown_open.get())
                    class=("border-accent-muted", move || dropdown_open.get())
                    class=("border-edge-subtle", move || !dropdown_open.get())
                    on:click=move |_| set_dropdown_open.update(|v| *v = !*v)
                    on:keydown=move |ev: web_sys::KeyboardEvent| {
                        if ev.key() == "Escape" && dropdown_open.get_untracked() {
                            set_dropdown_open.set(false);
                            ev.prevent_default();
                        }
                    }
                >
                    <span class="flex-1 min-w-0 text-left truncate text-fg-primary">
                        {move || face_blend_option(local_blend_mode.get()).label}
                    </span>
                    <svg
                        class="w-3 h-3 shrink-0 transition-transform duration-200"
                        class=("rotate-180", move || dropdown_open.get())
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                    >
                        <path d="m6 9 6 6 6-6" />
                    </svg>
                </button>

                <Show when=move || dropdown_open.get()>
                    <crate::components::control_panel::ControlDropdownDismissHandlers
                        class_name=dropdown_class.clone()
                        is_open=dropdown_open
                        set_open=set_dropdown_open
                    />
                    <leptos::portal::Portal>
                        <div class=move || dropdown_class_value.get_value()>
                            <div
                                class="fixed z-[9999]
                                       rounded-b-xl overflow-hidden
                                       bg-surface-overlay/98 backdrop-blur-xl
                                       border border-t-0 border-edge-subtle
                                       dropdown-glow animate-enter-down
                                       overflow-y-auto scrollbar-dropdown"
                                style=move || crate::components::control_panel::dropdown_panel_style(trigger_ref.get())
                                on:mousedown=|ev: leptos::ev::MouseEvent| ev.stop_propagation()
                            >
                                {FACE_BLEND_OPTIONS.iter().copied().map(|option| {
                                    let mode = option.mode;
                                    view! {
                                        <button
                                            type="button"
                                            class="dropdown-option w-full text-left px-3 py-[7px] text-xs cursor-pointer
                                                   flex items-center gap-2"
                                            class=("dropdown-option-active", move || local_blend_mode.get() == mode)
                                            class=("text-fg-tertiary", move || local_blend_mode.get() != mode)
                                            on:click=move |_| {
                                                set_mode.run(mode);
                                                set_dropdown_open.set(false);
                                            }
                                        >
                                            <span
                                                class="w-1 h-1 rounded-full shrink-0 transition-all duration-200"
                                                class=("bg-accent-muted", move || local_blend_mode.get() == mode)
                                                class=("scale-100", move || local_blend_mode.get() == mode)
                                                class=("opacity-100", move || local_blend_mode.get() == mode)
                                                class=("scale-0", move || local_blend_mode.get() != mode)
                                                class=("opacity-0", move || local_blend_mode.get() != mode)
                                            />
                                            <span class="truncate">{option.label}</span>
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                        </div>
                    </leptos::portal::Portal>
                </Show>
            </div>
        </div>
    }
}
