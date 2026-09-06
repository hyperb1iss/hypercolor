//! Face composition for a Screen surface in the Studio slide-over.
//!
//! How the assigned face composes with the live effect beneath it —
//! blend mode ("Cutout" reveals the effect through face transparency),
//! blend amount, and one-tap looks. This is the display target's
//! composition, a different axis from the per-layer blend inside the
//! face's own canvas, so it gets its own section above the layer stack.
//!
//! Also home to the default-face card: a screen with no scene layer of
//! its own may still be painting the display's stored default (spec 69
//! §3.6, a per-display preference that never lives in the scene). The
//! card names that face and offers to copy it into the scene or clear it.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_icons::Icon;

use hypercolor_leptos_ext::events::Input;
use hypercolor_types::layer::BlendMode;

use crate::api;
use crate::api::DisplayFaceScope;
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

    // The face assignment for the selected screen, shared with the Stage
    // chip and the default-face card. Refreshed after every commit here.
    let face = studio.screen_face;
    let refresh_face = studio.refresh_screen_face;
    let has_face = Signal::derive(move || face.get().is_some());

    // Local composition state, seeded from the server and pushed back
    // optimistically so the select and slider track the drag.
    let (blend_mode, set_blend_mode) = signal(BlendMode::Alpha);
    let (opacity, set_opacity) = signal(1.0_f32);
    // What this panel last sent. A refetch that merely echoes it back must
    // not overwrite a newer local value mid-drag; only a genuinely
    // different server value (another client, the CLI) re-seeds.
    let last_committed = StoredValue::new(None::<(BlendMode, u32)>);
    // Commits in flight. While any are pending, a refetch describes a state
    // this panel has already moved past (possibly an out-of-order older
    // commit's), so it must not reseed the local controls.
    let commits_in_flight = StoredValue::new(0_u32);
    let opacity_key = |amount: f32| (amount.clamp(0.0, 1.0) * 1000.0).round() as u32;
    Effect::new(move |_| {
        let target = face.get().and_then(|face| face.zone.display_target);
        let (mode, amount) = target.map_or((BlendMode::Alpha, 1.0), |target| {
            (target.blend_mode, target.opacity.clamp(0.0, 1.0))
        });
        let echo = last_committed.get_value() == Some((mode, opacity_key(amount)));
        if !echo && commits_in_flight.get_value() == 0 {
            set_blend_mode.set(mode);
            set_opacity.set(amount);
        }
    });

    let commit = Callback::new(move |(mode, amount): (Option<BlendMode>, Option<f32>)| {
        let Some(device_id) = display_device_id.get_untracked() else {
            return;
        };
        last_committed.set_value(Some((
            mode.unwrap_or_else(|| blend_mode.get_untracked()),
            opacity_key(amount.unwrap_or_else(|| opacity.get_untracked())),
        )));
        commits_in_flight.update_value(|count| *count += 1);
        let refresh_scene = studio.refresh_scene;
        spawn_local(async move {
            let result = api::update_display_face_composition(&device_id, mode, amount).await;
            commits_in_flight.try_update_value(|count| *count = count.saturating_sub(1));
            match result {
                Ok(_) => {
                    refresh_face.run(());
                    refresh_scene.run(());
                }
                Err(error) => {
                    refresh_face.run(());
                    toasts::toast_error(&format!("Face composition update failed: {error}"));
                }
            }
        });
    });

    let commit_opacity = leptos_use::use_debounce_fn(
        move || {
            if !blend_mode.get_untracked().blends_with_base() {
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
        let amount = if mode.blends_with_base() {
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

                    <Show when=move || blend_mode.get().blends_with_base()>
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

/// The Screen's default-face card. Shown while the screen paints its
/// stored default with no scene layer of its own, so Studio never reports
/// an empty stack over a face that is visibly running. "Use in this
/// scene" copies the default into the scene as an editable layer (the
/// scene layer then wins, per spec 69 precedence); "Clear default" drops
/// the preference, which blanks the screen in every scene that gives it
/// no face of its own.
#[component]
pub fn DefaultFaceCard(
    /// Physical display device backing the selected Screen surface.
    #[prop(into)]
    display_device_id: Signal<Option<String>>,
    /// Fired after the default is copied into the scene, so the host
    /// refetches the layer stack and the scene.
    on_layers_mutated: Callback<()>,
) -> impl IntoView {
    let studio = expect_context::<StudioContext>();
    let default_face = Memo::new(move |_| {
        studio
            .screen_face
            .get()
            .filter(|face| face.live_scope == DisplayFaceScope::Default)
    });
    let face_name = move || {
        default_face
            .get()
            .map(|face| face.effect.name)
            .unwrap_or_default()
    };
    let (busy, set_busy) = signal(false);
    let refresh_face = studio.refresh_screen_face;
    let refresh_scene = studio.refresh_scene;

    // Both actions resolve after an await, possibly once this card has
    // been swapped out by a selection change, so every post-await write
    // goes through the fallible forms and a disposed card stays silent.
    let promote = move |_| {
        let (Some(device_id), Some(face)) = (
            display_device_id.get_untracked(),
            default_face.get_untracked(),
        ) else {
            return;
        };
        set_busy.set(true);
        spawn_local(async move {
            match api::promote_default_face(&device_id, &face).await {
                Ok(_) => toasts::toast_success("Face copied into this scene"),
                Err(error) => {
                    toasts::toast_error(&format!(
                        "Could not copy the face into this scene: {error}"
                    ));
                }
            }
            set_busy.try_set(false);
            refresh_face.try_run(());
            on_layers_mutated.try_run(());
        });
    };
    let clear = move |_| {
        let Some(device_id) = display_device_id.get_untracked() else {
            return;
        };
        set_busy.set(true);
        spawn_local(async move {
            match api::delete_display_face(&device_id, DisplayFaceScope::Default).await {
                Ok(()) => toasts::toast_success("Default face cleared"),
                Err(error) => {
                    toasts::toast_error(&format!("Could not clear the default face: {error}"));
                }
            }
            set_busy.try_set(false);
            refresh_face.try_run(());
            refresh_scene.try_run(());
        });
    };

    view! {
        <Show when=move || default_face.get().is_some()>
            <section class="mt-4 rounded-xl border border-edge-subtle/70 bg-surface-overlay/50">
                <div class="flex items-center justify-between gap-3 border-b border-edge-subtle/60 px-4 py-3">
                    <div class="min-w-0">
                        <div class="flex items-center gap-2">
                            <span class="truncate text-sm font-semibold text-fg-primary">
                                {face_name}
                            </span>
                            <span
                                class="shrink-0 rounded-full border px-1.5 py-px text-[9px] font-semibold uppercase tracking-[0.14em]"
                                style="border-color: rgba(225, 53, 255, 0.35); color: rgba(225, 53, 255, 0.85)"
                            >
                                "Default"
                            </span>
                        </div>
                        <div class="text-[11px] text-fg-tertiary">
                            "This display's default face. It shows in every scene that gives the screen no face of its own."
                        </div>
                    </div>
                    <Icon
                        icon=LuMonitor
                        width="16px"
                        height="16px"
                        style="color: rgba(225, 53, 255, 0.72)"
                    />
                </div>
                <div class="flex flex-wrap items-center gap-2 px-4 py-3">
                    <button
                        type="button"
                        class="inline-flex items-center gap-1.5 rounded-md border border-accent-muted/60 bg-accent/10 px-2.5 py-1.5 text-[11px] font-medium text-fg-primary transition btn-press hover:bg-accent/20 disabled:cursor-wait disabled:opacity-60"
                        title="Copy this face into the scene as an editable layer"
                        disabled=move || busy.get()
                        on:click=promote
                    >
                        <Icon icon=LuCopy width="12px" height="12px" />
                        "Use in this scene"
                    </button>
                    <button
                        type="button"
                        class="inline-flex items-center gap-1.5 rounded-md border border-edge-subtle bg-surface-overlay/50 px-2.5 py-1.5 text-[11px] font-medium text-fg-secondary transition btn-press hover:text-fg-primary disabled:cursor-wait disabled:opacity-60"
                        title="Remove the default face from this display"
                        disabled=move || busy.get()
                        on:click=clear
                    >
                        <Icon icon=LuTrash2 width="12px" height="12px" />
                        "Clear default"
                    </button>
                    <p class="basis-full text-[10px] leading-relaxed text-fg-tertiary/80">
                        "Adding a layer to this screen also overrides the default here; the default keeps running in other scenes."
                    </p>
                </div>
            </section>
        </Show>
    }
}
