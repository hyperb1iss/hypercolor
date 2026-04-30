//! Device pairing modal — generic flow driven entirely by the backend descriptor.
//!
//! Renders physical-action flows (Hue/Nanoleaf) and credential-form flows (future WLED)
//! from the same component, with no backend-specific branches.

use std::collections::HashMap;
use std::time::Duration;

use hypercolor_leptos_ext::events::Input;
use hypercolor_leptos_ext::prelude::sleep;
use leptos::prelude::*;
use leptos_icons::Icon;

use crate::api::{
    self, DeviceAuthState, DeviceAuthSummary, DeviceSummary, PairDeviceStatus, PairingFlowKind,
};
use crate::app::DevicesContext;
use crate::components::device_card::{brand_colors, classify_brand};
use crate::icons::*;
use crate::toasts;

// ── Modal state machine ────────────────────────────────────────────────────

#[derive(Clone, PartialEq, Eq)]
enum PairingStage {
    /// Showing instructions, waiting for user to click the action button.
    Ready,
    /// Request in flight.
    Submitting,
    /// Backend says user must perform a physical action first.
    ActionRequired(String),
    /// Pairing succeeded.
    Success(String),
    /// Pairing failed.
    Error(String),
}

// ── Component ──────────────────────────────────────────────────────────────

#[component]
pub fn DevicePairingModal(
    device: DeviceSummary,
    #[prop(into)] on_close: Callback<()>,
    /// Fires with the device ID on successful pairing. The parent should
    /// only dismiss the modal if the ID matches the currently-shown device,
    /// to avoid a stale async response dismissing a modal opened for a
    /// different device.
    #[prop(into)]
    on_paired: Callback<String>,
) -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();
    let device_id = device.id.clone();
    let device_name = device.name.clone();
    let brand = classify_brand(&device);
    let (rgb, _) = brand_colors(&brand);

    let auth = device.auth.clone();
    let descriptor = auth.as_ref().and_then(|a| a.descriptor.clone());

    let (stage, set_stage) = signal(PairingStage::Ready);
    let (form_values, set_form_values) = signal(HashMap::<String, String>::new());

    // Determine if this is a credentials-form flow
    let has_fields = descriptor
        .as_ref()
        .map(|d| d.kind == PairingFlowKind::CredentialsForm && !d.fields.is_empty())
        .unwrap_or(false);

    let submit_pair = {
        let device_id = device_id.clone();
        let rgb = rgb.clone();
        move || {
            let device_id = device_id.clone();
            let _rgb = rgb.clone();
            let values = form_values.get_untracked();
            set_stage.set(PairingStage::Submitting);

            let devices_resource = ctx.devices_resource;
            leptos::task::spawn_local(async move {
                let req = api::PairDeviceRequest {
                    values,
                    activate_after_pair: true,
                };
                match api::pair_device(&device_id, &req).await {
                    Ok(resp) => match resp.status {
                        PairDeviceStatus::Paired | PairDeviceStatus::AlreadyPaired => {
                            let msg = resp.message.clone();
                            set_stage.set(PairingStage::Success(msg.clone()));
                            devices_resource.refetch();
                            toasts::toast_success(&msg);
                            sleep(Duration::from_millis(800)).await;
                            on_paired.run(device_id);
                        }
                        PairDeviceStatus::ActionRequired => {
                            set_stage.set(PairingStage::ActionRequired(resp.message));
                        }
                        PairDeviceStatus::InvalidInput => {
                            set_stage.set(PairingStage::Error(resp.message));
                        }
                    },
                    Err(error) => {
                        set_stage.set(PairingStage::Error(error.clone()));
                        toasts::toast_error(&error);
                    }
                }
            });
        }
    };

    let update_field = move |key: String, value: String| {
        set_form_values.update(|m| {
            m.insert(key, value);
        });
    };

    // If there's no descriptor, show a generic "pairing not available" state
    let Some(desc) = descriptor else {
        return view! {
            <ModalBackdrop on_close=on_close>
                <div class="text-center py-8">
                    <Icon icon=LuTriangleAlert width="32px" height="32px" style="color: rgba(255, 183, 77, 0.6)" />
                    <p class="text-sm text-fg-secondary mt-3">"Pairing is not available for this device."</p>
                    <button
                        class="mt-4 px-4 py-1.5 rounded-lg text-xs font-medium text-fg-tertiary bg-surface-overlay/40 border border-edge-subtle hover:bg-surface-hover/60 transition-colors"
                        on:click=move |_| on_close.run(())
                    >
                        "Close"
                    </button>
                </div>
            </ModalBackdrop>
        }
        .into_any();
    };

    let title = desc.title.clone();
    let instructions = desc.instructions.clone();
    let action_label = desc.action_label.clone();
    let fields = desc.fields.clone();

    view! {
        <ModalBackdrop on_close=on_close>
            // Header
            <div class="flex items-center gap-3 mb-4">
                <div
                    class="w-10 h-10 rounded-xl flex items-center justify-center"
                    style=format!("background: rgba({rgb}, 0.1); border: 1px solid rgba({rgb}, 0.15)")
                >
                    <Icon icon=LuKeyRound width="20px" height="20px" style=format!("color: rgba({rgb}, 0.7)") />
                </div>
                <div class="flex-1 min-w-0">
                    <h2 class="text-sm font-medium text-fg-primary">{title}</h2>
                    <p class="text-[11px] text-fg-tertiary truncate">{device_name}</p>
                </div>
                <button
                    class="w-7 h-7 rounded-lg flex items-center justify-center text-fg-tertiary hover:text-fg-secondary hover:bg-surface-hover/40 transition-colors"
                    on:click=move |_| on_close.run(())
                >
                    <Icon icon=LuX width="16px" height="16px" />
                </button>
            </div>

            // Instructions
            <div class="space-y-2 mb-4">
                {instructions.into_iter().enumerate().map(|(i, instruction)| {
                    let rgb = rgb.clone();
                    view! {
                        <div class="flex items-start gap-2.5">
                            <span
                                class="w-5 h-5 rounded-full flex items-center justify-center text-[10px] font-mono font-medium shrink-0 mt-0.5"
                                style=format!("background: rgba({rgb}, 0.1); color: rgba({rgb}, 0.7)")
                            >
                                {i + 1}
                            </span>
                            <p class="text-xs text-fg-secondary leading-relaxed">{instruction}</p>
                        </div>
                    }
                }).collect_view()}
            </div>

            // Credential form fields (CredentialsForm kind only)
            {has_fields.then(|| {
                let fields = fields.clone();
                view! {
                    <div class="space-y-2 mb-4">
                        {fields.into_iter().map(|field| {
                            let key = field.key.clone();
                            let key_for_handler = key.clone();
                            let input_type = if field.secret { "password" } else { "text" };
                            let placeholder = field.placeholder.clone().unwrap_or_default();
                            let label = field.label.clone();
                            let optional = field.optional;

                            view! {
                                <div>
                                    <label class="text-[10px] font-medium text-fg-tertiary uppercase tracking-wider mb-1 flex items-center gap-1">
                                        {if field.secret {
                                            view! { <Icon icon=LuLock width="9px" height="9px" /> }.into_any()
                                        } else {
                                            view! { <span /> }.into_any()
                                        }}
                                        {label}
                                        {optional.then(|| view! {
                                            <span class="text-fg-tertiary/30 normal-case">"(optional)"</span>
                                        })}
                                    </label>
                                    <input
                                        type=input_type
                                        placeholder=placeholder
                                        class="w-full bg-surface-overlay/60 border border-edge-subtle rounded-lg px-3 py-1.5 text-sm text-fg-primary
                                               placeholder-fg-tertiary/30 focus:outline-none focus:border-accent-muted transition-colors"
                                        on:input=move |ev| {
                                            let event = Input::from_event(ev);
                                            if let Some(value) = event.value_string() {
                                                update_field(key_for_handler.clone(), value);
                                            }
                                        }
                                    />
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }
            })}

            // Status message area
            {move || {
                let current_stage = stage.get();
                match current_stage {
                    PairingStage::ActionRequired(msg) => Some(view! {
                        <div class="mb-3 px-3 py-2 rounded-lg border"
                             style="background: rgba(255, 183, 77, 0.06); border-color: rgba(255, 183, 77, 0.15)">
                            <div class="flex items-center gap-2">
                                <Icon icon=LuTriangleAlert width="14px" height="14px" style="color: rgba(255, 183, 77, 0.7)" />
                                <p class="text-xs" style="color: rgba(255, 183, 77, 0.8)">{msg}</p>
                            </div>
                        </div>
                    }.into_any()),
                    PairingStage::Success(msg) => Some(view! {
                        <div class="mb-3 px-3 py-2 rounded-lg border"
                             style="background: rgba(80, 250, 123, 0.06); border-color: rgba(80, 250, 123, 0.15)">
                            <div class="flex items-center gap-2">
                                <Icon icon=LuCircleCheck width="14px" height="14px" style="color: rgba(80, 250, 123, 0.8)" />
                                <p class="text-xs" style="color: rgba(80, 250, 123, 0.8)">{msg}</p>
                            </div>
                        </div>
                    }.into_any()),
                    PairingStage::Error(msg) => Some(view! {
                        <div class="mb-3 px-3 py-2 rounded-lg border"
                             style="background: rgba(255, 99, 99, 0.06); border-color: rgba(255, 99, 99, 0.15)">
                            <div class="flex items-center gap-2">
                                <Icon icon=LuTriangleAlert width="14px" height="14px" style="color: rgba(255, 99, 99, 0.7)" />
                                <p class="text-xs" style="color: rgba(255, 99, 99, 0.8)">{msg}</p>
                            </div>
                        </div>
                    }.into_any()),
                    _ => None,
                }
            }}

            // Actions
            <div class="flex items-center gap-2">
                {
                    let action_label = action_label.clone();
                    let rgb = rgb.clone();
                    let submit = submit_pair.clone();
                    move || {
                        let current_stage = stage.get();
                        let is_submitting = current_stage == PairingStage::Submitting;
                        let is_success = matches!(current_stage, PairingStage::Success(_));
                        let label = if is_submitting {
                            "Pairing...".to_string()
                        } else if is_success {
                            "Paired!".to_string()
                        } else {
                            action_label.clone()
                        };
                        let r = rgb.clone();
                        let submit = submit.clone();

                        view! {
                            <button
                                class="flex-1 flex items-center justify-center gap-2 px-4 py-2 rounded-lg text-xs font-medium transition-all btn-press"
                                style=move || {
                                    if is_success {
                                        "background: rgba(80, 250, 123, 0.15); color: rgb(80, 250, 123); border: 1px solid rgba(80, 250, 123, 0.2)".to_string()
                                    } else {
                                        format!("background: rgba({r}, 0.12); color: rgb({r}); border: 1px solid rgba({r}, 0.2)")
                                    }
                                }
                                disabled=move || is_submitting || is_success
                                on:click=move |_| submit()
                            >
                                {if is_submitting {
                                    view! { <span class="animate-spin"><Icon icon=LuLoader width="14px" height="14px" /></span> }.into_any()
                                } else if is_success {
                                    view! { <Icon icon=LuCircleCheck width="14px" height="14px" /> }.into_any()
                                } else {
                                    view! { <Icon icon=LuZap width="14px" height="14px" /> }.into_any()
                                }}
                                {label}
                            </button>
                        }
                    }
                }
                <button
                    class="px-4 py-2 rounded-lg text-xs font-medium text-fg-tertiary bg-surface-overlay/40
                           border border-edge-subtle hover:bg-surface-hover/60 transition-colors"
                    on:click=move |_| on_close.run(())
                >
                    "Cancel"
                </button>
            </div>
        </ModalBackdrop>
    }
    .into_any()
}

// ── Forget confirmation dialog ─────────────────────────────────────────────

#[component]
pub fn ForgetCredentialsModal(
    device: DeviceSummary,
    #[prop(into)] on_close: Callback<()>,
    /// Fires with the device ID after credentials are successfully removed.
    /// The parent should only dismiss the modal if the ID matches the
    /// currently-shown device, to guard against stale async responses.
    #[prop(into)]
    on_forgot: Callback<String>,
) -> impl IntoView {
    let ctx = expect_context::<DevicesContext>();
    let device_id = device.id.clone();
    let device_name = device.name.clone();
    let (submitting, set_submitting) = signal(false);

    let do_forget = {
        let device_id = device_id.clone();
        move || {
            let device_id = device_id.clone();
            set_submitting.set(true);
            let devices_resource = ctx.devices_resource;
            leptos::task::spawn_local(async move {
                match api::unpair_device(&device_id).await {
                    Ok(resp) => {
                        toasts::toast_success(&resp.message);
                        devices_resource.refetch();
                        on_forgot.run(device_id);
                    }
                    Err(error) => {
                        toasts::toast_error(&error);
                        set_submitting.set(false);
                    }
                }
            });
        }
    };

    view! {
        <ModalBackdrop on_close=on_close>
            <div class="text-center">
                <div class="w-12 h-12 rounded-xl flex items-center justify-center mx-auto mb-3"
                     style="background: rgba(255, 99, 99, 0.08); border: 1px solid rgba(255, 99, 99, 0.12)">
                    <Icon icon=LuTrash2 width="22px" height="22px" style="color: rgba(255, 99, 99, 0.6)" />
                </div>
                <h2 class="text-sm font-medium text-fg-primary mb-1">"Forget credentials?"</h2>
                <p class="text-xs text-fg-tertiary mb-4">
                    "This will remove stored credentials for "
                    <span class="text-fg-secondary font-medium">{device_name}</span>
                    ". You'll need to pair the device again."
                </p>
                <div class="flex items-center gap-2 justify-center">
                    <button
                        class="flex-1 px-4 py-2 rounded-lg text-xs font-medium transition-all btn-press
                               border"
                        style="background: rgba(255, 99, 99, 0.1); color: rgb(255, 99, 99); border-color: rgba(255, 99, 99, 0.2)"
                        disabled=move || submitting.get()
                        on:click=move |_| do_forget()
                    >
                        {move || if submitting.get() { "Removing..." } else { "Forget credentials" }}
                    </button>
                    <button
                        class="px-4 py-2 rounded-lg text-xs font-medium text-fg-tertiary bg-surface-overlay/40
                               border border-edge-subtle hover:bg-surface-hover/60 transition-colors"
                        on:click=move |_| on_close.run(())
                    >
                        "Cancel"
                    </button>
                </div>
            </div>
        </ModalBackdrop>
    }
}

// ── Auth state badge for device cards ──────────────────────────────────────

/// Returns badge text, color RGB, and whether to show based on auth state.
pub fn auth_badge_info(auth: &Option<DeviceAuthSummary>) -> Option<(&'static str, &'static str)> {
    let summary = auth.as_ref()?;
    match summary.state {
        DeviceAuthState::Required => Some(("Pair required", "255, 183, 77")),
        DeviceAuthState::Error => Some(("Repair auth", "255, 99, 99")),
        _ => None,
    }
}

/// Whether the device needs pairing before it can be controlled.
pub fn needs_pairing(auth: &Option<DeviceAuthSummary>) -> bool {
    auth.as_ref()
        .map(|a| a.state == DeviceAuthState::Required || a.state == DeviceAuthState::Error)
        .unwrap_or(false)
}

// ── Shared modal backdrop ──────────────────────────────────────────────────

#[component]
fn ModalBackdrop(#[prop(into)] on_close: Callback<()>, children: Children) -> impl IntoView {
    view! {
        <div class="fixed inset-0 z-50 grid place-items-center p-4 animate-fade-in">
            // Backdrop
            <div
                class="absolute inset-0 bg-black/60 backdrop-blur-sm"
                on:click=move |_| on_close.run(())
            />
            // Modal panel — explicit width avoids flex/grid sizing quirks
            <div class="relative rounded-2xl border border-edge-subtle bg-surface-raised
                        shadow-2xl animate-scale-in p-5"
                 style="width: min(28rem, calc(100vw - 2rem)); box-shadow: 0 0 60px rgba(0,0,0,0.3), 0 0 30px rgba(225, 53, 255, 0.05)">
                {children()}
            </div>
        </div>
    }
}
