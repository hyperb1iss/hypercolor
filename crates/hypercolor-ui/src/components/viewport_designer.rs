//! Viewport Designer modal — authoring surface for effects that expose
//! a `ViewportRect` control (Web Viewport, Screen Cast).
//!
//! Scaffold-only for this commit — the module compiles and every type
//! in the public surface is reachable, but the `ViewportDesignerModal`
//! entry point is not yet wired into `control_panel`. That's the next
//! commit; carrying module-level `dead_code` allows here to keep the
//! shell landable as its own reviewable unit without red lint output.
#![allow(dead_code, clippy::too_many_arguments)]
//!
//! Implements Spec 46 Wave 1 MVP. The modal discriminates on effect
//! mode: Web Viewport gets URL + scroll + render-size controls,
//! Screen Cast gets the bare shared surface. Both share the overlay,
//! controls bar, numeric inputs, and apply/cancel machinery.
//!
//! This module is deliberately kept in a single file for Wave 1. If it
//! grows beyond ~1200 lines or the overlay/controls bar gain
//! independent test coverage, split into the directory structure the
//! spec sketches (overlay.rs, controls_bar.rs, etc.).
//!
//! Related files:
//!   - `api::effects::update_effect_controls` for the PATCH path with
//!     If-Match optimistic concurrency
//!   - `components::control_panel::viewport_picker` for the inline
//!     quick-adjust picker the modal complements (not replaces)

use leptos::ev;
use leptos::prelude::*;
use leptos_icons::Icon;
use serde_json::json;

use hypercolor_types::viewport::{FitMode, MIN_VIEWPORT_EDGE, ViewportRect};

use crate::api::effects::{UpdateControlsOutcome, update_effect_controls};
use crate::toasts::{toast_error, toast_info, toast_success};

/// Authoring mode the modal was opened against.
///
/// Drives which source pane + control groups render. Stored in the
/// draft so the rest of the tree can match on it without re-deriving
/// from effect metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewportDesignerMode {
    /// Web Viewport effect — Servo pane (+ future iframe), URL bar,
    /// scroll sliders, render-size inputs.
    WebViewport,
    /// Screen Cast effect — single live screen-capture pane, no URL,
    /// no scroll, no iframe.
    ScreenCast,
}

/// Shared draft fields present in every mode. Kept separate from
/// `ModeDraft` so the common controls bar can take this slice without
/// knowing which effect it's editing.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewportDraftCommon {
    pub viewport: ViewportRect,
    pub fit_mode: FitMode,
    pub brightness: f32,
    /// Server-side controls_version captured at open time or bumped
    /// after each successful PATCH. Used as `If-Match`.
    pub controls_version: u64,
}

/// Mode-specific draft state. A single flat struct would let invalid
/// combinations compile (a Screen Cast draft with `scroll_y`, a Web
/// Viewport draft with no URL); the enum makes those states
/// unrepresentable.
#[derive(Clone, Debug, PartialEq)]
pub enum ModeDraft {
    WebViewport {
        url: String,
        scroll_x: i32,
        scroll_y: i32,
        render_width: u32,
        render_height: u32,
    },
    ScreenCast,
}

impl ModeDraft {
    fn mode(&self) -> ViewportDesignerMode {
        match self {
            Self::WebViewport { .. } => ViewportDesignerMode::WebViewport,
            Self::ScreenCast => ViewportDesignerMode::ScreenCast,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ViewportDraft {
    pub common: ViewportDraftCommon,
    pub mode: ModeDraft,
}

impl ViewportDraft {
    /// True when any committed field differs from its open-time value.
    /// Drives the "Unsaved" badge and the cancel-confirmation dialog.
    fn is_dirty(&self, open_time: &ViewportDraft) -> bool {
        self != open_time
    }
}

/// Seed values threaded in from the caller: effect UUID, current live
/// control values, the render-group canvas aspect for the aspect-lock
/// toggle, and the effect-specific mode.
#[derive(Clone, Debug)]
pub struct ViewportDesignerContext {
    pub effect_id: String,
    pub effect_name: String,
    pub canvas_aspect: f32,
    pub initial_draft: ViewportDraft,
}

/// Terminal outcome of the modal. The caller wires it to whatever
/// refresh / reconciliation it needs on its side (e.g., pushing the
/// new draft values back into the inline picker's signals).
#[derive(Clone, Copy, Debug)]
pub enum ViewportDesignerResult {
    Applied,
    Cancelled,
}

#[component]
pub fn ViewportDesignerModal(
    context: ViewportDesignerContext,
    #[prop(into)] on_close: Callback<ViewportDesignerResult>,
) -> impl IntoView {
    // Two draft signals: the live editable one and a frozen snapshot of
    // the open-time values. The snapshot drives the "dirty" badge and
    // the cancel revert path — it must never change for the life of
    // the modal.
    let initial = context.initial_draft.clone();
    let effect_id = context.effect_id.clone();
    let (draft, set_draft) = signal(initial.clone());
    let open_time_draft = initial.clone();

    let mode = initial.mode.mode();
    let is_dirty = Signal::derive({
        let open_time = open_time_draft.clone();
        move || draft.with(|d| d.is_dirty(&open_time))
    });

    // Apply: send one final reconciling PATCH covering every committed
    // field. For Wave 1 we don't do mid-drag commits — the whole draft
    // ships on Apply. That's conservative but buys us the simplest
    // possible concurrency story for the first pass. § 6.1/6.2
    // throttled flows layer in once the shell is proven.
    let apply_pending = RwSignal::new(false);
    let apply = Callback::new({
        let effect_id = effect_id.clone();
        move |_: ()| {
            if apply_pending.get() {
                return;
            }
            apply_pending.set(true);
            let effect_id = effect_id.clone();
            leptos::task::spawn_local(async move {
                let snapshot = draft.get_untracked();
                let controls = draft_to_controls_payload(&snapshot);
                let outcome = update_effect_controls(
                    &effect_id,
                    &controls,
                    Some(snapshot.common.controls_version),
                )
                .await;
                apply_pending.set(false);
                match outcome {
                    Ok(UpdateControlsOutcome::Applied { new_version }) => {
                        set_draft.update(|d| d.common.controls_version = new_version);
                        toast_success("Viewport applied.");
                        on_close.run(ViewportDesignerResult::Applied);
                    }
                    Ok(UpdateControlsOutcome::Stale { current }) => {
                        // Rebase the draft's token so a follow-up
                        // Apply after the user's choice doesn't 412
                        // again on the same value. The user still
                        // sees the reconciliation hint — they can
                        // click Apply again to force-overwrite.
                        set_draft.update(|d| d.common.controls_version = current);
                        toast_info(
                            "Another client changed this effect. Re-apply to overwrite, or cancel to discard your edits.",
                        );
                    }
                    Err(err) => {
                        toast_error(&format!("Couldn't apply viewport: {err}"));
                    }
                }
            });
        }
    });

    let cancel = Callback::new(move |_: ()| {
        on_close.run(ViewportDesignerResult::Cancelled);
    });

    // Esc / backdrop-click close affordances. § 10.6 notes that iframe
    // focus capture breaks keydown propagation, but for the Wave 1
    // Servo-only modal there is no iframe so Esc on the backdrop-level
    // listener suffices. The explicit Close button + footer Cancel
    // still work regardless of focus location (clicks always bubble
    // to the modal root).
    let backdrop_click = move |ev: ev::MouseEvent| {
        // Only treat backdrop clicks as cancel — clicks on modal
        // children set `target != currentTarget`.
        if let Some(target) = ev.target()
            && let Some(current) = ev.current_target()
            && target == current
        {
            ev.prevent_default();
            cancel.run(());
        }
    };

    let handle_keydown = move |ev: ev::KeyboardEvent| {
        if ev.key() == "Escape" {
            ev.prevent_default();
            cancel.run(());
        }
    };

    view! {
        <div
            class="viewport-designer-backdrop"
            on:click=backdrop_click
            on:keydown=handle_keydown
            tabindex="-1"
        >
            <div class="viewport-designer-modal" role="dialog" aria-modal="true">
                <div class="viewport-designer-header">
                    <div class="viewport-designer-title">
                        <Icon icon=icondata::LuLayoutTemplate width="20" height="20" />
                        <span class="title-label">"Viewport Designer"</span>
                        <span class="title-effect">{context.effect_name.clone()}</span>
                    </div>
                    <div class="viewport-designer-header-actions">
                        {move || is_dirty.get().then(|| view! {
                            <span class="viewport-designer-unsaved" title="Unsaved changes">"● Unsaved"</span>
                        })}
                        <button
                            class="viewport-designer-close"
                            aria-label="Close"
                            on:click=move |_| cancel.run(())
                        >
                            "✕"
                        </button>
                    </div>
                </div>

                <div class="viewport-designer-body">
                    {move || match mode {
                        ViewportDesignerMode::WebViewport => {
                            view! { <WebViewportPaneStub draft=draft set_draft=set_draft /> }
                                .into_any()
                        }
                        ViewportDesignerMode::ScreenCast => {
                            view! { <ScreenCastPaneStub /> }.into_any()
                        }
                    }}
                </div>

                <div class="viewport-designer-footer">
                    <button
                        class="viewport-designer-button secondary"
                        on:click=move |_| cancel.run(())
                    >
                        "Cancel"
                    </button>
                    <button
                        class="viewport-designer-button primary"
                        disabled=move || apply_pending.get()
                        on:click=move |_| apply.run(())
                    >
                        {move || if apply_pending.get() { "Applying…" } else { "Apply" }}
                    </button>
                </div>
            </div>
        </div>
    }
}

/// Web Viewport stub pane. Wave 1 MVP: numeric rect inputs, scroll_y
/// slider, fit mode radio. Servo render-stream canvas + drag-resize
/// overlay + iframe pane land in follow-up commits.
#[component]
fn WebViewportPaneStub(
    draft: ReadSignal<ViewportDraft>,
    set_draft: WriteSignal<ViewportDraft>,
) -> impl IntoView {
    let url = Signal::derive(move || match draft.with(|d| d.mode.clone()) {
        ModeDraft::WebViewport { url, .. } => url,
        ModeDraft::ScreenCast => String::new(),
    });
    let scroll_y = Signal::derive(move || match draft.with(|d| d.mode.clone()) {
        ModeDraft::WebViewport { scroll_y, .. } => scroll_y,
        ModeDraft::ScreenCast => 0,
    });
    let fit_mode = Signal::derive(move || draft.with(|d| d.common.fit_mode));
    let viewport = Signal::derive(move || draft.with(|d| d.common.viewport));

    let update_mode = move |updater: Box<dyn FnOnce(&mut ModeDraft)>| {
        set_draft.update(|d| updater(&mut d.mode));
    };
    let update_viewport = move |next: ViewportRect| {
        set_draft.update(|d| d.common.viewport = next.clamp());
    };
    let update_fit = move |next: FitMode| {
        set_draft.update(|d| d.common.fit_mode = next);
    };

    view! {
        <div class="viewport-designer-content">
            <div class="viewport-designer-url-row">
                <label class="viewport-designer-field-label">"URL"</label>
                <input
                    class="viewport-designer-input"
                    type="text"
                    prop:value=move || url.get()
                    on:change=move |ev| {
                        let next = event_target_value(&ev);
                        update_mode(Box::new(move |mode| {
                            if let ModeDraft::WebViewport { url: slot, .. } = mode {
                                *slot = next;
                            }
                        }));
                    }
                />
            </div>

            <div class="viewport-designer-placeholder">
                <Icon icon=icondata::LuEye width="24" height="24" />
                <span>"Servo preview + drag-to-resize overlay land in the next commit."</span>
                <span class="viewport-designer-placeholder-hint">
                    "Use the numeric inputs below to position the crop in the meantime."
                </span>
            </div>

            <div class="viewport-designer-control-grid">
                <div class="viewport-designer-control-row">
                    <label>"Viewport"</label>
                    <NumericGrid value=viewport on_change=Callback::new(update_viewport) />
                </div>

                <div class="viewport-designer-control-row">
                    <label>"Fit mode"</label>
                    <FitModeRadio value=fit_mode on_change=Callback::new(update_fit) />
                </div>

                <div class="viewport-designer-control-row">
                    <label>
                        "Scroll Y: "
                        <span class="viewport-designer-scroll-value">
                            {move || format!("{}px", scroll_y.get())}
                        </span>
                    </label>
                    <input
                        class="viewport-designer-slider"
                        type="range"
                        min="0"
                        max="8000"
                        step="1"
                        prop:value=move || scroll_y.get().to_string()
                        on:input=move |ev| {
                            let raw = event_target_value(&ev);
                            let Ok(next) = raw.parse::<i32>() else {
                                return;
                            };
                            update_mode(Box::new(move |mode| {
                                if let ModeDraft::WebViewport { scroll_y: slot, .. } = mode {
                                    *slot = next;
                                }
                            }));
                        }
                    />
                </div>
            </div>
        </div>
    }
}

/// Screen Cast stub pane. Placeholder until we wire the screen-capture
/// preview subscription.
#[component]
fn ScreenCastPaneStub() -> impl IntoView {
    view! {
        <div class="viewport-designer-placeholder">
            <Icon icon=icondata::LuMonitor width="24" height="24" />
            <span>"Screen Capture pane wiring lands in the next commit."</span>
        </div>
    }
}

/// Four-field numeric grid for the viewport rect. Clamped on commit so
/// the draft never holds invalid values.
#[component]
fn NumericGrid(
    #[prop(into)] value: Signal<ViewportRect>,
    on_change: Callback<ViewportRect>,
) -> impl IntoView {
    let commit = move |next: ViewportRect| {
        on_change.run(next);
    };
    let bind_field = move |axis: NumericAxis| {
        Callback::new(move |next: f32| {
            let current = value.get();
            let updated = match axis {
                NumericAxis::X => ViewportRect::new(next, current.y, current.width, current.height),
                NumericAxis::Y => ViewportRect::new(current.x, next, current.width, current.height),
                NumericAxis::Width => {
                    ViewportRect::new(current.x, current.y, next.max(MIN_VIEWPORT_EDGE), current.height)
                }
                NumericAxis::Height => ViewportRect::new(
                    current.x,
                    current.y,
                    current.width,
                    next.max(MIN_VIEWPORT_EDGE),
                ),
            };
            commit(updated);
        })
    };

    view! {
        <div class="viewport-designer-numeric-grid">
            <NumericField label="x" value=Signal::derive(move || value.get().x) on_change=bind_field(NumericAxis::X) />
            <NumericField label="y" value=Signal::derive(move || value.get().y) on_change=bind_field(NumericAxis::Y) />
            <NumericField label="w" value=Signal::derive(move || value.get().width) on_change=bind_field(NumericAxis::Width) />
            <NumericField label="h" value=Signal::derive(move || value.get().height) on_change=bind_field(NumericAxis::Height) />
        </div>
    }
}

#[derive(Clone, Copy)]
enum NumericAxis {
    X,
    Y,
    Width,
    Height,
}

#[component]
fn NumericField(
    label: &'static str,
    #[prop(into)] value: Signal<f32>,
    on_change: Callback<f32>,
) -> impl IntoView {
    view! {
        <div class="viewport-designer-numeric-field">
            <span class="viewport-designer-numeric-label">{label}</span>
            <input
                class="viewport-designer-numeric-input"
                type="number"
                step="0.01"
                min="0"
                max="1"
                prop:value=move || format!("{:.3}", value.get())
                on:change=move |ev| {
                    let raw = event_target_value(&ev);
                    if let Ok(parsed) = raw.parse::<f32>() {
                        on_change.run(parsed.clamp(0.0, 1.0));
                    }
                }
            />
        </div>
    }
}

#[component]
fn FitModeRadio(
    #[prop(into)] value: Signal<FitMode>,
    on_change: Callback<FitMode>,
) -> impl IntoView {
    let variants = [
        ("Cover", FitMode::Cover),
        ("Contain", FitMode::Contain),
        ("Stretch", FitMode::Stretch),
    ];

    view! {
        <div class="viewport-designer-fit-radio" role="radiogroup">
            {variants.map(|(label, mode)| {
                let current = value;
                view! {
                    <button
                        class="viewport-designer-fit-chip"
                        role="radio"
                        aria-checked=move || (current.get() == mode).to_string()
                        on:click=move |_| on_change.run(mode)
                    >
                        {label}
                    </button>
                }
            }).collect_view()}
        </div>
    }
}

/// Serialize the draft into the JSON shape the PATCH endpoint expects.
///
/// Only fields the mode actually carries end up in the payload; scroll
/// axes and render-size are omitted for Screen Cast, render-size stays
/// at the current value for Web Viewport unless the user changed it.
fn draft_to_controls_payload(draft: &ViewportDraft) -> serde_json::Value {
    let mut controls = serde_json::Map::new();
    controls.insert(
        "viewport".to_owned(),
        json!({
            "rect": {
                "x": draft.common.viewport.x,
                "y": draft.common.viewport.y,
                "width": draft.common.viewport.width,
                "height": draft.common.viewport.height,
            }
        }),
    );
    controls.insert(
        "fit_mode".to_owned(),
        json!({ "enum": fit_mode_label(draft.common.fit_mode) }),
    );
    controls.insert(
        "brightness".to_owned(),
        json!({ "float": draft.common.brightness }),
    );
    if let ModeDraft::WebViewport {
        url,
        scroll_x,
        scroll_y,
        render_width,
        render_height,
    } = &draft.mode
    {
        controls.insert("url".to_owned(), json!({ "text": url }));
        controls.insert("scroll_x".to_owned(), json!({ "float": *scroll_x as f32 }));
        controls.insert("scroll_y".to_owned(), json!({ "float": *scroll_y as f32 }));
        controls.insert(
            "render_width".to_owned(),
            json!({ "float": *render_width as f32 }),
        );
        controls.insert(
            "render_height".to_owned(),
            json!({ "float": *render_height as f32 }),
        );
    }
    serde_json::Value::Object(controls)
}

fn fit_mode_label(fit: FitMode) -> &'static str {
    match fit {
        FitMode::Cover => "Cover",
        FitMode::Contain => "Contain",
        FitMode::Stretch => "Stretch",
    }
}
