//! The dashboard's status line: one calm row under the title.
//!
//! `● Running · up 11h 40m · 23 devices · 60 fps`. The dot is the only
//! color and carries the daemon's health in status tokens; every value is
//! secondary text. Devices, fps, and the pause state ride the WebSocket;
//! the uptime is anchored to the shared status snapshot and advanced on
//! each metrics message. The preview widget lives in the shared
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
    Stopped,
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
            Self::Stopped => "Stopped",
            Self::Offline => "Offline",
        }
    }

    #[must_use]
    pub const fn tone(self) -> HealthTone {
        match self {
            Self::Running => HealthTone::Success,
            Self::Paused | Self::Connecting => HealthTone::Warning,
            Self::Stopped | Self::Offline => HealthTone::Error,
        }
    }
}

/// Health from the socket, the render loop's state in the last status
/// snapshot (`None` before one arrives), and the live output pause.
#[must_use]
pub const fn daemon_health(
    connection: ConnectionState,
    render_loop_running: Option<bool>,
    output_paused: bool,
) -> DaemonHealth {
    match (connection, render_loop_running, output_paused) {
        (ConnectionState::Connecting, _, _) => DaemonHealth::Connecting,
        (ConnectionState::Disconnected | ConnectionState::Error, _, _) => DaemonHealth::Offline,
        (ConnectionState::Connected, Some(false), _) => DaemonHealth::Stopped,
        (ConnectionState::Connected, _, true) => DaemonHealth::Paused,
        (ConnectionState::Connected, _, false) => DaemonHealth::Running,
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

#[component]
pub(super) fn StatusLine() -> impl IntoView {
    let ws = expect_context::<WsContext>();
    let daemon_status = expect_context::<DaemonStatusContext>();
    let status = daemon_status.status;

    let health = Memo::new(move |_| {
        daemon_health(
            ws.connection_state.get(),
            status.with(|status| status.as_ref().map(|status| status.running)),
            ws.output_paused.get(),
        )
    });
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
        })
    });

    let separator = || {
        view! { <span class="text-fg-tertiary/50" aria-hidden="true">"·"</span> }
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
                {separator()}
                <span class="shrink-0">{format!("up {uptime}")}</span>
            })}
            {move || devices.get().map(|count| view! {
                {separator()}
                <A
                    href=crate::route_ui::route_href("/devices")
                    attr:class="shrink-0 rounded transition-colors hover:text-fg-primary"
                    attr:title="Open Devices"
                >
                    {device_count_label(count)}
                </A>
            })}
            {move || fps.get().filter(|fps| *fps > 0.0).map(|fps| view! {
                {separator()}
                <span class="shrink-0">{fps_label(fps)}</span>
            })}
        </div>
    }
}
