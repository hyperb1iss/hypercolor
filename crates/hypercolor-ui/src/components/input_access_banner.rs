//! Input-access remediation banner — shown above the active effect's
//! preview/controls when an interactive effect can't receive host input.
//!
//! Two states, decided by [`crate::input_access::input_access_remedy`]:
//! consent off (offer the one-click `input.enabled` toggle) and consent on
//! but every input node denied (show the udev install command). Interactive
//! detection reuses [`effect_wants_interaction`], the same predicate that
//! gates the browser-preview injection toggle.
//!
//! Freshness rides signals, never timers: the status `LocalResource`
//! refetches when the active effect changes, when the daemon socket
//! reconnects (`connection_generation`), and after a successful Enable
//! action bumps the manual epoch. Non-interactive effects skip the fetch
//! entirely.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api;
use crate::app::{EffectsContext, WsContext};
use crate::async_helpers::{spawn_mutation, toast_on_err, with_rollback};
use crate::components::canvas_preview::effect_wants_interaction;
use crate::icons::{LuKeyboard, LuX};
use crate::input_access::{InputAccessRemedy, input_access_remedy};

/// The remediation command for the denied-devices case. Rendered as a
/// selectable mono chip (no clipboard helper exists in this crate).
pub const UDEV_INSTALL_COMMAND: &str = "sudo just udev-install";

/// Inline banner surfacing input-access remediation for the active effect.
#[component]
pub fn InputAccessBanner() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let fx = expect_context::<EffectsContext>();

    let wants_input = Memo::new(move |_| {
        fx.active_effect_id.get().is_some_and(|id| {
            fx.effects_index.with(|effects| {
                effects
                    .iter()
                    .find(|entry| entry.effect.id == id)
                    .is_some_and(|entry| effect_wants_interaction(&entry.effect))
            })
        })
    });

    // Bumped after a successful Enable so the banner clears on the very
    // next status fetch instead of waiting for an external event.
    let refetch_epoch = RwSignal::new(0_u64);

    let input_status = LocalResource::new(move || {
        let generation = ws.connection_generation.get();
        let epoch = refetch_epoch.get();
        let active_id = fx.active_effect_id.get();
        let wants = wants_input.get();
        async move {
            let _ = (generation, epoch, active_id);
            if !wants {
                return None;
            }
            api::fetch_status().await.ok().map(|status| status.input)
        }
    });

    let remedy = Memo::new(move |_| {
        input_status
            .get()
            .flatten()
            .and_then(|input| input_access_remedy(wants_input.get(), &input))
    });

    // Dismissal is keyed on (effect, remedy) so waving off the notice for
    // one effect doesn't mute it for the next interactive one, and a state
    // change (consent granted, devices still denied) re-surfaces it.
    let dismissed = RwSignal::new(None::<(String, InputAccessRemedy)>);
    let visible_remedy = Memo::new(move |_| {
        let kind = remedy.get()?;
        let key = (fx.active_effect_id.get().unwrap_or_default(), kind);
        (dismissed.get() != Some(key)).then_some(kind)
    });

    let enabling = RwSignal::new(false);
    let on_enable = move |_| {
        if enabling.get_untracked() {
            return;
        }
        enabling.set(true);
        spawn_mutation(
            async move { api::set_config_value("input.enabled", &serde_json::json!(true)).await },
            move |()| {
                enabling.set(false);
                refetch_epoch.update(|epoch| *epoch += 1);
            },
            with_rollback(
                move || enabling.set(false),
                toast_on_err("Enable input access failed"),
            ),
        );
    };

    let on_dismiss = move |_| {
        if let Some(kind) = remedy.get_untracked() {
            let key = (
                fx.active_effect_id.get_untracked().unwrap_or_default(),
                kind,
            );
            dismissed.set(Some(key));
        }
    };

    view! {
        {move || visible_remedy.get().map(|kind| {
            let is_consent = kind == InputAccessRemedy::EnableConsent;
            view! {
                <div
                    class="glass-subtle rounded-xl border border-edge-subtle px-4 py-3 mb-3"
                    role="status"
                >
                    <div class="flex items-start gap-3">
                        <div class="mt-0.5 shrink-0 text-accent">
                            <Icon icon=LuKeyboard width="14px" height="14px" />
                        </div>
                        <div class="min-w-0 flex-1 text-sm leading-5 text-fg-secondary">
                            {if is_consent {
                                view! {
                                    <span>
                                        "This effect reacts to your keyboard and mouse. \
                                         Turn on Input Access to let it respond."
                                    </span>
                                }.into_any()
                            } else {
                                view! {
                                    <span>
                                        "Input access is on, but Hypercolor can't read your \
                                         input devices. Install the udev rules and replug:"
                                        <code class="ml-1.5 select-all rounded-md border \
                                                     border-edge-subtle bg-surface-sunken/60 \
                                                     px-1.5 py-0.5 font-mono text-[11px] \
                                                     text-fg-primary whitespace-nowrap">
                                            {UDEV_INSTALL_COMMAND}
                                        </code>
                                    </span>
                                }.into_any()
                            }}
                        </div>
                        {is_consent.then(|| view! {
                            <button
                                type="button"
                                class="shrink-0 rounded-lg bg-accent px-3 py-1.5 text-[11px] \
                                       font-medium text-white transition hover:bg-accent-hover \
                                       disabled:cursor-wait disabled:opacity-60"
                                aria-label="Enable input access"
                                disabled=move || enabling.get()
                                on:click=on_enable
                            >
                                {move || if enabling.get() {
                                    "Enabling..."
                                } else {
                                    "Enable input access"
                                }}
                            </button>
                        })}
                        <button
                            type="button"
                            class="shrink-0 rounded-md p-1 text-fg-tertiary transition-colors \
                                   hover:text-fg-primary"
                            aria-label="Dismiss input access notice"
                            on:click=on_dismiss
                        >
                            <Icon icon=LuX width="12px" height="12px" />
                        </button>
                    </div>
                </div>
            }
        })}
    }
}
