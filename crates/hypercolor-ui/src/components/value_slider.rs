//! One number on a track: drag it, step it with the keys, type it, or
//! send it back to its default.
//!
//! Every slider a layer or an effect shows in the inspector is this one.
//! The track is a native range input, so the arrow keys already step it;
//! Shift with an arrow takes [`COARSE_STEPS`] at once. The readout opens
//! into a field that takes a typed value. A value away from its default
//! grows a reset button, and a double-click on the track or the label
//! does the same.

use std::time::Duration;

use hypercolor_leptos_ext::events::{Change, Input};
use hypercolor_leptos_ext::prelude::{TimeoutHandle, set_timeout};
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::icons::LuRotateCcw;

/// How many steps Shift with an arrow key takes.
pub const COARSE_STEPS: i16 = 10;

/// How long a settled value waits for a later one before it is written.
/// A held arrow key, or the two clicks of a double-click, then cost one
/// write instead of one each.
const SETTLE: Duration = Duration::from_millis(140);

/// The accent a slider glows in when its owner names none.
const ACCENT_RGB: &str = "225, 53, 255";

/// The values a slider can hold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ValueRange {
    pub min: f32,
    pub max: f32,
    pub step: f32,
}

impl ValueRange {
    /// `value` held inside the range.
    #[must_use]
    pub fn hold(self, value: f32) -> f32 {
        value.clamp(self.min.min(self.max), self.min.max(self.max))
    }

    /// `value` held inside the range and on its step grid, which is where
    /// the track itself can put the thumb.
    #[must_use]
    pub fn settle(self, value: f32) -> f32 {
        let (low, high) = (self.min.min(self.max), self.min.max(self.max));
        let held = self.hold(value);
        if !self.step.is_finite() || self.step <= 0.0 {
            return held;
        }
        let steps = ((held - self.min) / self.step).round();
        (self.min + steps * self.step).clamp(low, high)
    }

    /// `value` moved by a number of steps, up for positive.
    #[must_use]
    pub fn stepped(self, value: f32, steps: i16) -> f32 {
        self.settle(value + f32::from(steps) * self.step)
    }
}

/// How a slider writes its value, and reads one typed back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ValueFormat {
    decimals: usize,
    /// What the stored value is multiplied by to be shown.
    scale: f32,
    suffix: &'static str,
}

impl ValueFormat {
    /// The value itself, to `decimals` places.
    #[must_use]
    pub const fn plain(decimals: usize) -> Self {
        Self {
            decimals,
            scale: 1.0,
            suffix: "",
        }
    }

    /// A 0 to 1 value as a whole percentage.
    #[must_use]
    pub const fn percent() -> Self {
        Self {
            decimals: 0,
            scale: 100.0,
            suffix: "%",
        }
    }

    /// As many places as the step needs, so the readout matches the
    /// track's granularity instead of jittering between widths.
    #[must_use]
    pub fn for_step(step: f32) -> Self {
        Self::plain(if step >= 1.0 {
            0
        } else if step >= 0.1 {
            1
        } else if step >= 0.01 {
            2
        } else {
            3
        })
    }

    /// The same format with a unit after the number.
    #[must_use]
    pub const fn suffixed(mut self, suffix: &'static str) -> Self {
        self.suffix = suffix;
        self
    }

    /// The readout: the number and its unit.
    #[must_use]
    pub fn write(self, value: f32) -> String {
        format!("{}{}", self.entry(value), self.suffix)
    }

    /// What the entry field starts with: the number alone.
    #[must_use]
    pub fn entry(self, value: f32) -> String {
        format!("{:.*}", self.decimals, value * self.scale)
    }

    /// `value` to the places the readout writes: as fine as a typed
    /// entry gets.
    #[must_use]
    pub fn rounded(self, value: f32) -> f32 {
        self.read(&self.entry(value)).unwrap_or(value)
    }

    /// Whether the readout writes two values differently.
    #[must_use]
    pub fn tells_apart(self, left: f32, right: f32) -> bool {
        self.entry(left) != self.entry(right)
    }

    /// The value a typed entry names, or `None` for anything that is not
    /// a number. The unit may be typed or left off, and a comma counts as
    /// a decimal point.
    #[must_use]
    pub fn read(self, text: &str) -> Option<f32> {
        let text = text.trim();
        let number = if self.suffix.is_empty() {
            text
        } else {
            text.strip_suffix(self.suffix).unwrap_or(text).trim_end()
        };
        let shown = number.replace(',', ".").parse::<f32>().ok()?;
        let value = shown / self.scale;
        value.is_finite().then_some(value)
    }
}

/// A labelled slider with a typed readout and a way back to its default.
///
/// `on_input` hears every value the slider passes through, for an owner
/// that follows a drag live. `on_commit` hears each value the slider
/// comes to rest on, for an owner whose writes are too heavy to follow a
/// drag. A slider takes whichever its owner listens to, or both.
#[component]
pub fn ValueSlider(
    #[prop(into)] label: Signal<String>,
    /// The value as its owner has it.
    #[prop(into)]
    value: Signal<f32>,
    range: ValueRange,
    format: ValueFormat,
    /// Where reset returns to. Without one the slider offers no reset.
    #[prop(optional_no_strip)]
    default: Option<f32>,
    #[prop(optional_no_strip)] on_input: Option<Callback<f32>>,
    #[prop(optional_no_strip)] on_commit: Option<Callback<f32>>,
    /// An icon ahead of the label, and its inline style.
    #[prop(optional_no_strip)]
    icon: Option<(icondata::Icon, String)>,
    #[prop(optional_no_strip)] accent_rgb: Option<String>,
    #[prop(optional_no_strip)] tooltip: Option<String>,
    label_class: &'static str,
) -> impl IntoView {
    let accent_rgb = accent_rgb.unwrap_or_else(|| ACCENT_RGB.to_owned());
    let home = default.map(|default| range.hold(default));

    // What the slider shows: its owner's value, or one the owner has not
    // heard of or answered yet.
    let shown = RwSignal::new(value.get_untracked());
    let pending = StoredValue::new(None::<f32>);
    let timer: StoredValue<Option<TimeoutHandle>, LocalStorage> = StoredValue::new_local(None);
    let editing = RwSignal::new(false);
    let draft = RwSignal::new(String::new());
    let refused = RwSignal::new(false);
    let track = NodeRef::<leptos::html::Input>::new();
    let entry = NodeRef::<leptos::html::Input>::new();

    // The owner's value is adopted when it moves, not each time it is
    // restated, and never over a value still waiting to be written.
    Effect::new(move |seen: Option<f32>| {
        let next = value.get();
        let moved = seen.is_none_or(|seen| (seen - next).abs() > f32::EPSILON);
        if moved && pending.with_value(Option::is_none) {
            shown.set(next);
        }
        next
    });

    let disarm = move || {
        timer.try_update_value(|timer| {
            if let Some(mut timer) = timer.take() {
                timer.cancel();
            }
        });
    };
    let write = move || {
        disarm();
        if let Some(value) = pending.try_update_value(Option::take).flatten()
            && let Some(on_commit) = on_commit
        {
            let _ = on_commit.try_run(value);
        }
    };
    let pass = move |value: f32| {
        shown.set(value);
        if let Some(on_input) = on_input {
            on_input.run(value);
        }
    };
    let rest_soon = move |value: f32| {
        if on_commit.is_none() {
            return;
        }
        pending.set_value(Some(value));
        disarm();
        timer.set_value(Some(set_timeout(SETTLE, write)));
    };
    let rest_now = move |value: f32| {
        pending.set_value(Some(value));
        write();
    };
    // A value still waiting when the slider leaves is written, not lost.
    on_cleanup(write);

    let changed =
        Memo::new(move |_| home.is_some_and(|home| format.tells_apart(shown.get(), home)));
    let reset = move || {
        let Some(home) = home else {
            return;
        };
        // The clicks of a double-click each moved the thumb; what they
        // left waiting is dropped for the one value that matters.
        pending.set_value(None);
        disarm();
        let away = format.tells_apart(shown.get_untracked(), home)
            || format.tells_apart(value.get_untracked(), home);
        if away {
            pass(home);
            rest_now(home);
        } else {
            shown.set(home);
        }
    };

    let begin = move || {
        draft.set(format.entry(shown.get_untracked()));
        refused.set(false);
        editing.set(true);
    };
    // Take the typed value. Answers whether the entry closed: one that is
    // not a number stays open and is marked, and changes nothing.
    let accept = move || {
        let Some(typed) = format.read(&draft.get_untracked()) else {
            refused.set(true);
            return false;
        };
        // A typed value keeps the precision the readout shows, finer than
        // the track's steps when its owner allows.
        let next = format.rounded(range.hold(typed));
        editing.set(false);
        if format.tells_apart(next, shown.get_untracked()) {
            pass(next);
            rest_now(next);
        }
        true
    };
    let to_track = move || {
        if let Some(track) = track.get_untracked() {
            let _ = track.focus();
        }
    };
    // The entry opens focused with its number selected, so typing
    // replaces it. The frame's wait lets the field take its value first.
    Effect::new(move |_| {
        if entry.get().is_some() {
            request_animation_frame(move || {
                if let Some(entry) = entry.get_untracked() {
                    let _ = entry.focus();
                    entry.select();
                }
            });
        }
    });

    let badge_style =
        format!("color: rgba({accent_rgb}, 0.9); background: rgba({accent_rgb}, 0.08)");
    let reset_title = home.map(|home| format!("Reset to {}", format.write(home)));

    view! {
        <div
            class="group flex items-center gap-2.5 rounded-lg px-3 py-2 transition-colors duration-200 hover:bg-surface-hover/20"
            title=tooltip.unwrap_or_default()
            style=format!("--glow-rgb: {accent_rgb}")
        >
            {icon.map(|(icon, style)| view! { <Icon icon=icon width="15px" height="15px" style=style /> })}
            <label class=label_class on:dblclick=move |_| reset()>{move || label.get()}</label>
            <input
                node_ref=track
                type="range"
                class="slider-silk min-w-0 flex-1 cursor-pointer"
                aria-label=move || label.get()
                min=range.min
                max=range.max
                step=range.step
                prop:value=move || shown.get()
                on:input=move |event| {
                    if let Some(value) = Input::from_event(event).value::<f32>() {
                        pass(value);
                    }
                }
                on:change=move |event| {
                    if let Some(value) = Change::from_event(event).value::<f32>() {
                        shown.set(value);
                        rest_soon(value);
                    }
                }
                on:keydown=move |event: web_sys::KeyboardEvent| {
                    if !event.shift_key() {
                        return;
                    }
                    let steps = match event.key().as_str() {
                        "ArrowRight" | "ArrowUp" => COARSE_STEPS,
                        "ArrowLeft" | "ArrowDown" => -COARSE_STEPS,
                        _ => return,
                    };
                    event.prevent_default();
                    let next = range.stepped(shown.get_untracked(), steps);
                    pass(next);
                    rest_soon(next);
                }
                on:dblclick=move |_| reset()
            />
            {reset_title.map(|title| view! {
                <button
                    type="button"
                    class="-mx-1 flex h-4 w-4 shrink-0 items-center justify-center rounded text-fg-tertiary transition-colors hover:text-fg-primary btn-press"
                    class:invisible=move || !changed.get()
                    title=title
                    aria-label=move || format!("Reset {}", label.get())
                    data-value-reset=""
                    on:click=move |_| {
                        reset();
                        to_track();
                    }
                >
                    <Icon icon=LuRotateCcw width="11px" height="11px" />
                </button>
            })}
            <Show
                when=move || editing.get()
                fallback=move || view! {
                    <button
                        type="button"
                        class="w-[42px] shrink-0 cursor-text rounded px-1.5 py-0.5 text-right font-mono text-[10px] tabular-nums transition-shadow hover:ring-1 hover:ring-accent/40"
                        style=badge_style.clone()
                        title="Type a value"
                        aria-label=move || format!("{}: {}", label.get(), format.write(shown.get()))
                        data-value-readout=""
                        on:click=move |_| begin()
                    >
                        {move || format.write(shown.get())}
                    </button>
                }
            >
                <input
                    node_ref=entry
                    type="text"
                    inputmode="decimal"
                    autocomplete="off"
                    spellcheck="false"
                    class="w-[42px] shrink-0 rounded bg-surface-sunken px-1.5 py-0.5 text-right font-mono text-[10px] tabular-nums text-fg-primary outline-none ring-1"
                    class=("ring-accent/60", move || !refused.get())
                    class=("ring-status-error", move || refused.get())
                    aria-label=move || format!("{} value", label.get())
                    aria-invalid=move || refused.get().to_string()
                    data-value-entry=""
                    prop:value=move || draft.get()
                    on:input=move |event| {
                        draft.set(event_target_value(&event));
                        refused.set(false);
                    }
                    on:keydown=move |event: web_sys::KeyboardEvent| {
                        let steps = if event.shift_key() { COARSE_STEPS } else { 1 };
                        match event.key().as_str() {
                            "Enter" => {
                                event.prevent_default();
                                if accept() {
                                    to_track();
                                }
                            }
                            // The entry closes; the panel it sits in does not.
                            "Escape" => {
                                event.prevent_default();
                                event.stop_propagation();
                                editing.set(false);
                                to_track();
                            }
                            key @ ("ArrowUp" | "ArrowDown") => {
                                event.prevent_default();
                                let from = format
                                    .read(&draft.get_untracked())
                                    .unwrap_or_else(|| shown.get_untracked());
                                let steps = if key == "ArrowUp" { steps } else { -steps };
                                draft.set(format.entry(range.stepped(from, steps)));
                                refused.set(false);
                            }
                            _ => {}
                        }
                    }
                    on:blur=move |_| {
                        // Leaving the entry takes what it holds; an entry
                        // that is not a number is let go.
                        if editing.try_get_untracked() == Some(true) && !accept() {
                            editing.set(false);
                        }
                    }
                />
            </Show>
        </div>
    }
}
