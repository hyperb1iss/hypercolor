//! Face composition for a Screen surface in the Studio slide-over.
//!
//! How the assigned face composes with the live effect beneath it —
//! blend mode ("Cutout" reveals the effect through face transparency),
//! blend amount, and one-tap looks. This is the display target's
//! composition, a different axis from the per-layer blend inside the
//! face's own canvas, so it gets its own section above the layer stack.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use hypercolor_leptos_ext::events::Input;
use hypercolor_types::scene::DisplayFaceBlendMode;

use crate::api;
use crate::components::section_label::{LabelSize, LabelTone, label_class};
use crate::components::silk_select::SilkSelect;
use crate::face_blend::{
    FACE_BLEND_PRESETS, face_blend_option, face_blend_select_options, face_blend_value,
    parse_face_blend,
};
use crate::icons::*;
use crate::toasts;

use super::StudioContext;

/// Debounce for the blend-amount slider — matches the Displays page.
const OPACITY_DEBOUNCE_MS: f64 = 90.0;

/// The Screen surface's face-composition section. Renders nothing while
/// the screen has no face assigned; there is no composition without one.
#[component]
pub fn ScreenCompositionSection(
    /// Physical display device backing the selected Screen surface.
    #[prop(into)]
    display_device_id: Signal<Option<String>>,
) -> impl IntoView {
    let studio = expect_context::<StudioContext>();

    // The face assignment for the selected screen. Retargets when the
    // selection moves and refreshes after every composition commit.
    let (face_tick, set_face_tick) = signal(0_u64);
    let face_resource = LocalResource::new(move || {
        let _ = face_tick.get();
        let device_id = display_device_id.get();
        async move {
            match device_id {
                Some(device_id) => api::fetch_display_face(&device_id).await,
                None => Ok(None),
            }
        }
    });
    let face = Signal::derive(move || face_resource.get().and_then(Result::ok).flatten());
    let has_face = Signal::derive(move || face.get().is_some());

    // Local composition state, seeded from the server and pushed back
    // optimistically so the select and slider track the drag.
    let (blend_mode, set_blend_mode) = signal(DisplayFaceBlendMode::Alpha);
    let (opacity, set_opacity) = signal(1.0_f32);
    Effect::new(move |_| {
        let target = face.get().and_then(|face| face.group.display_target);
        if let Some(target) = target {
            set_blend_mode.set(target.blend_mode);
            set_opacity.set(target.opacity.clamp(0.0, 1.0));
        } else {
            set_blend_mode.set(DisplayFaceBlendMode::Alpha);
            set_opacity.set(1.0);
        }
    });

    let commit = Callback::new(
        move |(mode, amount): (Option<DisplayFaceBlendMode>, Option<f32>)| {
            let Some(device_id) = display_device_id.get_untracked() else {
                return;
            };
            let refresh_scene = studio.refresh_scene;
            spawn_local(async move {
                match api::update_display_face_composition(&device_id, mode, amount).await {
                    Ok(_) => {
                        set_face_tick.update(|tick| *tick = tick.wrapping_add(1));
                        refresh_scene.run(());
                    }
                    Err(error) => {
                        set_face_tick.update(|tick| *tick = tick.wrapping_add(1));
                        toasts::toast_error(&format!("Face composition update failed: {error}"));
                    }
                }
            });
        },
    );

    let commit_opacity = leptos_use::use_debounce_fn(
        move || {
            if !blend_mode.get_untracked().blends_with_effect() {
                return;
            }
            commit.run((None, Some(opacity.get_untracked())));
        },
        OPACITY_DEBOUNCE_MS,
    );
    // `Callback` is `Copy`; the debounced closure itself is not, and view
    // closures must stay `Fn`.
    let on_opacity_input = Callback::new(move |event: web_sys::Event| {
        if let Some(raw) = Input::from_event(event).value::<f32>() {
            set_opacity.set((raw / 100.0).clamp(0.0, 1.0));
            commit_opacity();
        }
    });

    let on_blend_change = Callback::new(move |value: String| {
        let mode = parse_face_blend(&value);
        set_blend_mode.set(mode);
        let amount = if mode.blends_with_effect() {
            opacity.get_untracked()
        } else {
            1.0
        };
        commit.run((Some(mode), Some(amount)));
    });

    view! {
        <Show when=move || has_face.get()>
            <section class="mt-4 rounded-xl border border-edge-subtle/70 bg-surface-overlay/50">
                <div class="flex items-center justify-between gap-3 border-b border-edge-subtle/60 px-4 py-3">
                    <div>
                        <div class="text-sm font-semibold text-fg-primary">"Face Composition"</div>
                        <div class="text-[11px] text-fg-tertiary">
                            "How the face layers over the live effect"
                        </div>
                    </div>
                    <Icon
                        icon=LuSlidersHorizontal
                        width="16px"
                        height="16px"
                        style="color: rgba(255, 106, 193, 0.72)"
                    />
                </div>
                <div class="space-y-3 px-4 py-4">
                    <div class="space-y-1.5">
                        <span class=label_class(LabelSize::Micro, LabelTone::Default)>
                            "Blend mode"
                        </span>
                        <SilkSelect
                            value=Signal::derive(move || {
                                face_blend_value(blend_mode.get()).to_owned()
                            })
                            options=Signal::derive(face_blend_select_options)
                            on_change=on_blend_change
                            placeholder="Blend mode"
                            class="border border-edge-subtle bg-surface-sunken/55 px-3 py-2 text-xs text-fg-primary"
                            label_class="font-medium"
                        />
                        <p class="px-1 text-[10px] leading-relaxed text-fg-tertiary/80">
                            {move || face_blend_option(blend_mode.get()).blurb}
                        </p>
                    </div>

                    <Show when=move || blend_mode.get().blends_with_effect()>
                        <label class="grid grid-cols-[88px_1fr_44px] items-center gap-2 text-[10px] font-mono uppercase tracking-wide text-fg-tertiary/75">
                            <span>"Blend amount"</span>
                            <input
                                type="range"
                                min="0"
                                max="100"
                                step="1"
                                class="w-full accent-coral"
                                prop:value=move || format!("{:.0}", opacity.get() * 100.0)
                                on:input=move |event| on_opacity_input.run(event)
                            />
                            <span class="text-right tabular-nums">
                                {move || format!("{:.0}%", opacity.get() * 100.0)}
                            </span>
                        </label>
                    </Show>

                    <div class="space-y-1.5">
                        <span class=label_class(LabelSize::Micro, LabelTone::Default)>
                            "Quick looks"
                        </span>
                        <div class="flex flex-wrap gap-1.5">
                            {FACE_BLEND_PRESETS
                                .iter()
                                .copied()
                                .map(|preset| {
                                    let is_active = move || {
                                        blend_mode.get() == preset.mode
                                            && (opacity.get() - preset.opacity).abs() <= 0.01
                                    };
                                    view! {
                                        <button
                                            type="button"
                                            class="inline-flex items-center rounded-md border px-2.5 py-1 text-[10px] font-medium transition btn-press"
                                            class=("border-coral/50", is_active)
                                            class=("bg-coral/12", is_active)
                                            class=("text-coral", is_active)
                                            class=("border-edge-subtle", move || !is_active())
                                            class=("bg-surface-overlay/50", move || !is_active())
                                            class=("text-fg-secondary", move || !is_active())
                                            on:click=move |_| {
                                                set_blend_mode.set(preset.mode);
                                                set_opacity.set(preset.opacity);
                                                commit.run((Some(preset.mode), Some(preset.opacity)));
                                            }
                                        >
                                            {preset.label}
                                        </button>
                                    }
                                })
                                .collect_view()}
                        </div>
                    </div>
                </div>
            </section>
        </Show>
    }
}
