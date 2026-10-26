//! The dashboard's status line: one calm row under the title.
//!
//! `● Running · up 11h 40m · 23 devices · 60 fps`. The dot is the only
//! color and carries the daemon's health in status tokens; every value is
//! secondary text. Health, devices, and fps ride the WebSocket; the uptime
//! is anchored to the shared status snapshot and advanced on each metrics
//! message. The preview widget lives in the shared
//! `components::preview_cabinet` module.

use hypercolor_leptos_ext::prelude::now_ms;
use leptos::prelude::*;
use leptos_router::components::A;

use crate::app::{DaemonStatusContext, WsContext};
use crate::live_uptime::{advanced_uptime_seconds, format_uptime};
use crate::ws::messages::ConnectionState;

use super::fps_display::stabilize_fps_for_display;

/// The daemon's health, as the status line's dot reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonHealth {
    Running,
    Paused,
    Connecting,
    Offline,
}

/// The status token a health state paints its dot with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthTone {
    Success,
    Warning,
    Error,
}

impl DaemonHealth {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Paused => "Paused",
            Self::Connecting => "Connecting",
            Self::Offline => "Offline",
        }
    }

    #[must_use]
    pub const fn tone(self) -> HealthTone {
        match self {
            Self::Running => HealthTone::Success,
            Self::Paused | Self::Connecting => HealthTone::Warning,
            Self::Offline => HealthTone::Error,
        }
    }
}

/// Health from the two live signals the socket carries: its own state,
/// and the output pause its events report. The daemon's render loop only
/// stops on shutdown, which drops the socket, so a connected daemon is
/// running or paused.
#[must_use]
pub const fn daemon_health(connection: ConnectionState, output_paused: bool) -> DaemonHealth {
    match (connection, output_paused) {
        (ConnectionState::Connecting, _) => DaemonHealth::Connecting,
        (ConnectionState::Disconnected | ConnectionState::Error, _) => DaemonHealth::Offline,
        (ConnectionState::Connected, true) => DaemonHealth::Paused,
        (ConnectionState::Connected, false) => DaemonHealth::Running,
    }
}

/// "1 device", "23 devices".
#[must_use]
pub fn device_count_label(count: usize) -> String {
    if count == 1 {
        "1 device".to_owned()
    } else {
        format!("{count} devices")
    }
}

/// The delivered frame rate, whole frames per second.
#[must_use]
pub fn fps_label(fps: f64) -> String {
    format!("{fps:.0} fps")
}

/// The values the status line shows after the health word, in line order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusValue {
    Uptime,
    Devices,
    Fps,
}

/// Classes for the separator in front of `value`, given the values the
/// line shows. Phones hide the health word, so there the first value
/// shown leads the line and its separator hides with the word; wider
/// screens keep every separator.
#[must_use]
pub fn separator_class(value: StatusValue, shown: &[StatusValue]) -> &'static str {
    if shown.first() == Some(&value) {
        "text-fg-tertiary/50 max-md:hidden"
    } else {
        "text-fg-tertiary/50"
    }
}

#[component]
pub(super) fn StatusLine() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let daemon_status = expect_context::<DaemonStatusContext>();
    let status = daemon_status.status;

    let health =
        Memo::new(move |_| daemon_health(ws.connection_state.get(), ws.output_paused.get()));
    // The snapshot's uptime, advanced from when the snapshot landed.
    let uptime = Memo::new(move |_| {
        ws.metrics_tick.track();
        let received_at = daemon_status.received_at_ms.get();
        status.with(|status| {
            status.as_ref().map(|status| {
                format_uptime(advanced_uptime_seconds(
                    status.uptime_seconds,
                    received_at,
                    now_ms(),
                ))
            })
        })
    });
    let devices = Memo::new(move |_| {
        ws.metrics
            .with(|metrics| metrics.as_ref().map(|metrics| metrics.devices.connected))
            .or_else(|| status.with(|status| status.as_ref().map(|status| status.device_count)))
    });
    let fps = Memo::new(move |_| {
        ws.metrics.with(|metrics| {
            metrics
                .as_ref()
                .map(|metrics| stabilize_fps_for_display(metrics.fps.delivered, metrics.fps.target))
                .filter(|fps| *fps > 0.0)
        })
    });

    let shown = Memo::new(move |_| {
        [
            uptime.with(Option::is_some).then_some(StatusValue::Uptime),
            devices
                .with(Option::is_some)
                .then_some(StatusValue::Devices),
            fps.with(Option::is_some).then_some(StatusValue::Fps),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
    });
    let separator = move |value: StatusValue| {
        view! {
            <span
                class=move || shown.with(|shown| separator_class(value, shown))
                aria-hidden="true"
            >
                "·"
            </span>
        }
    };

    view! {
        <div class="flex min-w-0 items-center gap-2 text-[12px] text-fg-secondary tabular-nums max-md:text-[11px]">
            <span
                class="h-2 w-2 shrink-0 rounded-full"
                class=("bg-status-success", move || health.get().tone() == HealthTone::Success)
                class=("bg-status-warning", move || health.get().tone() == HealthTone::Warning)
                class=("bg-status-error", move || health.get().tone() == HealthTone::Error)
                aria-hidden="true"
            />
            // Phones keep the dot and drop the word; readers keep both.
            <span class="shrink-0 max-md:sr-only">{move || health.get().label()}</span>
            {move || uptime.get().map(|uptime| view! {
                {separator(StatusValue::Uptime)}
                <span class="shrink-0">{format!("up {uptime}")}</span>
            })}
            {move || devices.get().map(|count| view! {
                {separator(StatusValue::Devices)}
                <A
                    href=crate::route_ui::route_href("/devices")
                    attr:class="shrink-0 rounded transition-colors hover:text-fg-primary"
                    attr:title="Open Devices"
                >
                    {device_count_label(count)}
                </A>
            })}
            {move || fps.get().map(|fps| view! {
                {separator(StatusValue::Fps)}
                <span class="shrink-0">{fps_label(fps)}</span>
            })}
        </div>
    }
}
