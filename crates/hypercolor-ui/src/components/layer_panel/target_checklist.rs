//! The checklist a layer's target is chosen from: the whole zone, or the
//! zone's outputs grouped by device.
//!
//! The list edits a draft and commits nothing. Its host decides where it
//! shows (a popover beside the stack, a sheet on a phone) and what "done"
//! means. The rules of the draft live in [`super::scope`].

use std::collections::HashSet;

use leptos::prelude::*;
use leptos_icons::Icon;

use super::scope::{Checked, Target, TargetGroup, TargetOutput, target_groups};
use crate::icons::*;

const ROW: &str = "flex w-full min-w-0 items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left \
     text-[12px] text-fg-secondary transition-colors hover:bg-surface-hover/40 \
     hover:text-fg-primary pointer-coarse:min-h-11 max-md:py-2.5 max-md:text-sm";

/// The checklist. `on_change` hears each new draft; `on_hover` the
/// outputs the row under the pointer stands for, and none when it leaves.
#[component]
pub fn TargetChecklist(
    #[prop(into)] draft: Signal<Target>,
    #[prop(into)] outputs: Signal<Vec<TargetOutput>>,
    on_change: Callback<Target>,
    #[prop(optional)] on_hover: Option<Callback<HashSet<String>>>,
) -> impl IntoView {
    let groups = Memo::new(move |_| outputs.with(|outputs| target_groups(outputs)));
    let missing = Memo::new(move |_| {
        outputs.with(|outputs| draft.with(|draft| draft.missing(outputs).len()))
    });
    let has_missing = move || missing.get() != 0;
    let toggle = move |ids: Vec<String>| {
        let mut next = draft.get_untracked();
        next.toggle(&ids);
        on_change.run(next);
    };
    let hover = move |ids: Vec<String>| {
        if let Some(on_hover) = on_hover {
            on_hover.run(ids.into_iter().collect());
        }
    };
    let whole = move || draft.with(Target::is_whole_zone);

    view! {
        <div role="group" aria-label="Applies to" on:mouseleave=move |_| hover(Vec::new())>
            <button
                type="button"
                role="radio"
                aria-checked=move || whole().to_string()
                class=ROW
                data-target-item=""
                on:mouseenter=move |_| hover(Vec::new())
                on:click=move |_| on_change.run(Target::WholeZone)
            >
                <span
                    class="flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-full border transition-colors"
                    class=("border-accent", whole)
                    class=("border-edge-default", move || !whole())
                    aria-hidden="true"
                >
                    <Show when=whole>
                        <span class="h-1.5 w-1.5 rounded-full bg-accent" />
                    </Show>
                </span>
                <span class="min-w-0 flex-1 truncate" class=("text-fg-primary", whole)>
                    "Whole zone"
                </span>
                <span class="shrink-0 text-[10px] text-fg-tertiary">"Every output"</span>
            </button>
            <div class="my-1 h-px bg-edge-subtle/60" role="separator" />
            <For
                each=move || groups.get()
                key=|group| (group.device_id.clone(), group.ids())
                children=move |group: TargetGroup| {
                    let single = group.outputs.len() == 1;
                    let ids = StoredValue::new(group.ids());
                    let probe = StoredValue::new(group.clone());
                    let checked = Memo::new(move |_| {
                        draft.with(|draft| probe.with_value(|group| draft.checked(group)))
                    });
                    let count = group.outputs.len();
                    view! {
                        <button
                            type="button"
                            role="checkbox"
                            aria-checked=move || match checked.get() {
                                Checked::All => "true",
                                Checked::Some => "mixed",
                                Checked::None => "false",
                            }
                            class=ROW
                            data-target-item=""
                            on:mouseenter=move |_| hover(ids.get_value())
                            on:click=move |_| toggle(ids.get_value())
                        >
                            <CheckBox checked=checked />
                            <span
                                class="min-w-0 flex-1 truncate"
                                class=("text-fg-primary", move || checked.get() != Checked::None)
                            >
                                {group.device_name.clone()}
                            </span>
                            {(!single).then(|| view! {
                                <span class="shrink-0 text-[10px] tabular-nums text-fg-tertiary">
                                    {format!("{count} outputs")}
                                </span>
                            })}
                        </button>
                        {(!single).then(|| view! {
                            <div class="ml-[1.0625rem] border-l border-edge-subtle/60 pl-1.5">
                                {group
                                    .outputs
                                    .iter()
                                    .map(|output| {
                                        let id = StoredValue::new(output.id.clone());
                                        let held = Memo::new(move |_| {
                                            draft.with(|draft| id.with_value(|id| draft.holds(id)))
                                        });
                                        let state = Memo::new(move |_| {
                                            if held.get() { Checked::All } else { Checked::None }
                                        });
                                        view! {
                                            <button
                                                type="button"
                                                role="checkbox"
                                                aria-checked=move || held.get().to_string()
                                                class=ROW
                                                data-target-item=""
                                                on:mouseenter=move |_| hover(vec![id.get_value()])
                                                on:click=move |_| toggle(vec![id.get_value()])
                                            >
                                                <CheckBox checked=state />
                                                <span
                                                    class="min-w-0 flex-1 truncate"
                                                    class=("text-fg-primary", move || held.get())
                                                >
                                                    {output.name.clone()}
                                                </span>
                                            </button>
                                        }
                                    })
                                    .collect_view()}
                            </div>
                        })}
                    }
                }
            />
            // Outputs the layer still names that have left the zone: kept
            // until told otherwise, since they light it again on return.
            <Show when=has_missing>
                <div class="my-1 h-px bg-edge-subtle/60" role="separator" />
                <div class="flex items-center gap-2 px-2.5 py-1.5 text-[11px] text-status-warning">
                    <span class="flex shrink-0">
                        <Icon icon=LuTriangleAlert width="12px" height="12px" />
                    </span>
                    <span class="min-w-0 flex-1">
                        {move || match missing.get() {
                            1 => "1 output is not in this zone".to_owned(),
                            count => format!("{count} outputs are not in this zone"),
                        }}
                    </span>
                    <button
                        type="button"
                        class="shrink-0 rounded px-1.5 py-0.5 text-[11px] font-medium text-fg-secondary transition-colors hover:bg-surface-hover/40 hover:text-fg-primary"
                        title="Stop keeping this layer for outputs that have left"
                        data-target-item=""
                        on:click=move |_| {
                            let mut next = draft.get_untracked();
                            outputs.with_untracked(|outputs| next.drop_missing(outputs));
                            on_change.run(next);
                        }
                    >
                        "Forget"
                    </button>
                </div>
            </Show>
        </div>
    }
}

/// A checkbox glyph: empty, ticked, or part of a device.
#[component]
fn CheckBox(#[prop(into)] checked: Signal<Checked>) -> impl IntoView {
    let on = move || checked.get() != Checked::None;
    view! {
        <span
            class="flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded border transition-colors"
            class=("border-accent", on)
            class=("bg-accent", on)
            class=("text-white", on)
            class=("border-edge-default", move || !on())
            aria-hidden="true"
        >
            {move || match checked.get() {
                Checked::All => view! { <Icon icon=LuCheck width="10px" height="10px" /> }.into_any(),
                Checked::Some => view! { <Icon icon=LuMinus width="10px" height="10px" /> }.into_any(),
                Checked::None => ().into_any(),
            }}
        </span>
    }
}
