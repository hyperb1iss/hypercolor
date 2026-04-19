//! Viewport Designer modal — authoring surface for effects that expose
//! a `ViewportRect` control (Web Viewport, Screen Cast).
#![allow(clippy::too_many_arguments)]
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
use leptos::portal::Portal;
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
    /// Target LED canvas aspect ratio. Reserved for the aspect-lock
    /// toggle in the next commit; carry it through now so callers
    /// don't have to change their context construction later.
    #[allow(dead_code)]
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

    // Esc closes the modal. § 10.6 notes iframe focus capture breaks
    // keydown propagation, but Wave 1 has no iframe — the fallback
    // close affordances (backdrop click + × button + Cancel) still
    // work regardless of focus.
    let handle_keydown = move |ev: ev::KeyboardEvent| {
        if ev.key() == "Escape" {
            ev.prevent_default();
            cancel.run(());
        }
    };

    view! {
        // Portal to <body> so the modal escapes any ancestor stacking
        // context / overflow clip (the controls column uses transforms
        // for scroll animation, which would otherwise trap a fixed
        // overlay inside the column — same class of bug the dropdown
        // menus hit, solved the same way).
        <Portal>
            // Fixed full-viewport overlay — z-50 sits above the rest of
            // the UI chrome, `grid place-items-center` centers the panel
            // without depending on flex math inside the caller. Matches
            // the `ModalBackdrop` pattern used by `device_pairing_modal`.
            <div
                class="fixed inset-0 z-50 grid place-items-center p-4 animate-fade-in"
                on:keydown=handle_keydown
                tabindex="-1"
            >
            // Click-absorbing backdrop — clicking the dimmed area cancels.
            // The modal panel below stops click propagation so buttons inside
            // never fall through to this handler.
            <div
                class="absolute inset-0 bg-black/60 backdrop-blur-sm"
                on:click=move |ev| {
                    ev.prevent_default();
                    cancel.run(());
                }
            />
            // Modal panel.
            <div
                class="relative flex flex-col overflow-hidden rounded-2xl border border-edge-subtle
                       bg-surface-raised shadow-2xl animate-scale-in"
                style="width: min(72rem, calc(100vw - 2rem)); max-height: calc(100vh - 4rem); \
                       box-shadow: 0 0 80px rgba(0, 0, 0, 0.45), 0 0 40px rgba(225, 53, 255, 0.08);"
                role="dialog"
                aria-modal="true"
            >
                // Header
                <div class="flex items-center justify-between gap-3 border-b border-edge-subtle px-5 py-3">
                    <div class="flex items-center gap-2.5 min-w-0">
                        <div class="text-accent-primary shrink-0">
                            <Icon icon=icondata::LuLayoutTemplate width="20" height="20" />
                        </div>
                        <span class="text-[11px] font-mono uppercase tracking-[0.14em] text-fg-tertiary">
                            "Viewport Designer"
                        </span>
                        <span class="truncate text-sm text-fg-primary font-medium">
                            {context.effect_name.clone()}
                        </span>
                    </div>
                    <div class="flex items-center gap-2 shrink-0">
                        {move || is_dirty.get().then(|| view! {
                            <span
                                class="rounded-full border border-accent-muted/40 bg-accent-muted/10 px-2 py-0.5
                                       text-[10px] font-mono uppercase tracking-[0.12em] text-accent-primary"
                                title="Unsaved changes"
                            >
                                "● Unsaved"
                            </span>
                        })}
                        <button
                            class="rounded-lg border border-edge-subtle/60 bg-surface-sunken/60 px-2.5 py-1
                                   text-sm text-fg-secondary transition-all duration-150 hover:scale-[1.04]
                                   hover:border-accent-muted/40 hover:text-accent-primary"
                            aria-label="Close"
                            on:click=move |_| cancel.run(())
                        >
                            "✕"
                        </button>
                    </div>
                </div>

                // Body — scrolls internally if the panel is smaller than
                // the viewport. Padding matches the existing ModalBackdrop.
                <div class="min-h-0 flex-1 overflow-y-auto px-5 py-4">
                    {move || match mode {
                        ViewportDesignerMode::WebViewport => view! {
                            <WebViewportPaneStub draft=draft set_draft=set_draft />
                        }.into_any(),
                        ViewportDesignerMode::ScreenCast => view! {
                            <ScreenCastPaneStub />
                        }.into_any(),
                    }}
                </div>

                // Footer
                <div class="flex items-center justify-end gap-2 border-t border-edge-subtle bg-surface-sunken/40 px-5 py-3">
                    <button
                        class="rounded-xl border border-edge-subtle/80 bg-surface-sunken/60 px-3.5 py-1.5
                               text-xs font-medium text-fg-secondary transition-all duration-150
                               hover:border-edge-strong hover:text-fg-primary"
                        on:click=move |_| cancel.run(())
                    >
                        "Cancel"
                    </button>
                    <button
                        class="rounded-xl border border-accent-muted/50 bg-accent-muted/15 px-4 py-1.5
                               text-xs font-medium text-accent-primary transition-all duration-150
                               hover:border-accent-muted hover:bg-accent-muted/25
                               disabled:cursor-not-allowed disabled:opacity-60"
                        disabled=move || apply_pending.get()
                        on:click=move |_| apply.run(())
                    >
                        {move || if apply_pending.get() { "Applying…" } else { "Apply" }}
                    </button>
                </div>
            </div>
            </div>
        </Portal>
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
        <div class="flex flex-col gap-4">
            <div class="flex items-center gap-3">
                <label class="shrink-0 text-[10px] font-mono uppercase tracking-[0.14em] text-fg-tertiary">
                    "URL"
                </label>
                <input
                    class="flex-1 rounded-xl border border-edge-subtle bg-surface-sunken px-3 py-2 text-xs
                           text-fg-primary transition-all duration-150
                           placeholder-fg-tertiary/40 focus:border-accent-muted focus:outline-none"
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

            <div class="flex flex-col items-center justify-center gap-2 rounded-2xl border border-edge-subtle
                        bg-surface-sunken/50 px-4 py-10 text-fg-tertiary">
                <Icon icon=icondata::LuEye width="24" height="24" />
                <span class="text-sm text-fg-secondary">
                    "Servo preview + drag-to-resize overlay land in the next commit."
                </span>
                <span class="text-[11px] text-fg-tertiary/80">
                    "Use the numeric inputs below to position the crop in the meantime."
                </span>
            </div>

            <div class="grid grid-cols-[140px_1fr] items-center gap-3">
                <label class="text-[10px] font-mono uppercase tracking-[0.14em] text-fg-tertiary">
                    "Viewport"
                </label>
                <NumericGrid value=viewport on_change=Callback::new(update_viewport) />

                <label class="text-[10px] font-mono uppercase tracking-[0.14em] text-fg-tertiary">
                    "Fit mode"
                </label>
                <FitModeRadio value=fit_mode on_change=Callback::new(update_fit) />

                <label class="text-[10px] font-mono uppercase tracking-[0.14em] text-fg-tertiary">
                    {move || format!("Scroll Y · {}px", scroll_y.get())}
                </label>
                <input
                    class="h-2 w-full accent-accent-primary"
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
    }
}

/// Screen Cast stub pane. Placeholder until we wire the screen-capture
/// preview subscription.
#[component]
fn ScreenCastPaneStub() -> impl IntoView {
    view! {
        <div class="flex flex-col items-center justify-center gap-2 rounded-2xl border border-edge-subtle
                    bg-surface-sunken/50 px-4 py-12 text-fg-tertiary">
            <Icon icon=icondata::LuMonitor width="24" height="24" />
            <span class="text-sm text-fg-secondary">
                "Screen Capture pane wiring lands in the next commit."
            </span>
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
                NumericAxis::Width => ViewportRect::new(
                    current.x,
                    current.y,
                    next.max(MIN_VIEWPORT_EDGE),
                    current.height,
                ),
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
        <div class="grid grid-cols-4 gap-2">
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
        <label class="flex flex-col gap-1">
            <span class="text-[9px] font-mono uppercase tracking-[0.12em] text-fg-tertiary">
                {label}
            </span>
            <input
                class="rounded-lg border border-edge-subtle bg-surface-sunken px-2 py-1 text-xs
                       text-fg-primary transition-all duration-150
                       focus:border-accent-muted focus:outline-none"
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
        </label>
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
        <div class="flex items-center gap-1.5" role="radiogroup">
            {variants.map(|(label, mode)| {
                let current = value;
                view! {
                    <button
                        class=move || {
                            let active = current.get() == mode;
                            let base = "rounded-lg border px-3 py-1 text-[11px] font-medium uppercase \
                                        tracking-[0.12em] transition-all duration-150";
                            if active {
                                format!("{base} border-accent-muted/60 bg-accent-muted/15 text-accent-primary")
                            } else {
                                format!("{base} border-edge-subtle bg-surface-sunken/50 text-fg-secondary \
                                         hover:border-edge-strong hover:text-fg-primary")
                            }
                        }
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
