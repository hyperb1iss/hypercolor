//! `hyper status` -- display current system state.

use std::future::Future;
use std::time::Duration;

use anyhow::Result;
use clap::Args;

use hypercolor_types::api::system::{SystemResource, SystemStatus};

use crate::client::{DaemonClient, DaemonEventSubscription};
use crate::output::{OutputContext, OutputFormat, Painter, wire_label};

/// Show current system state: running effect, devices, FPS, audio capture.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Live-updating status (re-renders on state change).
    #[arg(long)]
    pub watch: bool,

    /// Minimum render interval for --watch mode in seconds.
    #[arg(long, default_value = "1")]
    pub interval: f64,
}

trait StatusWatchClient {
    type Events: StatusWatchEvents;

    async fn subscribe_status_events(&self) -> Result<Self::Events>;
    async fn status_snapshot(&self) -> Result<SystemStatus>;
}

trait StatusWatchEvents {
    async fn next_status_event(&mut self) -> Result<Option<serde_json::Value>>;
    async fn close(self);
}

impl StatusWatchClient for DaemonClient {
    type Events = DaemonEventSubscription;

    async fn subscribe_status_events(&self) -> Result<Self::Events> {
        self.subscribe_events().await
    }

    async fn status_snapshot(&self) -> Result<SystemStatus> {
        let system: SystemResource = self.get("/system").await?;
        status_from_system(system)
    }
}

impl StatusWatchEvents for DaemonEventSubscription {
    async fn next_status_event(&mut self) -> Result<Option<serde_json::Value>> {
        self.next_event().await
    }

    async fn close(self) {
        self.close().await;
    }
}

#[derive(Debug)]
struct StatusWatchError {
    exit_code: i32,
    source: anyhow::Error,
}

impl StatusWatchError {
    fn connection(source: anyhow::Error) -> Self {
        Self {
            exit_code: 2,
            source,
        }
    }

    fn stream(source: anyhow::Error) -> Self {
        Self {
            exit_code: 1,
            source,
        }
    }
}

impl std::fmt::Display for StatusWatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("status watch failed")
    }
}

impl std::error::Error for StatusWatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

pub(crate) fn exit_code_for_error(error: &anyhow::Error) -> Option<i32> {
    error
        .downcast_ref::<StatusWatchError>()
        .map(|error| error.exit_code)
}

/// Execute the `status` subcommand.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable.
pub async fn execute(args: &StatusArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    if args.watch {
        return Box::pin(watch_status(args, client, ctx)).await;
    }

    let response = status_from_system(client.get("/system").await?)?;
    render_status(&response, ctx)?;

    Ok(())
}

fn status_from_system(system: SystemResource) -> Result<SystemStatus> {
    system
        .status
        .ok_or_else(|| anyhow::anyhow!("System status requires daemon read access"))
}

async fn watch_status(args: &StatusArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    Box::pin(watch_status_until(
        args,
        client,
        ctx,
        tokio::signal::ctrl_c(),
    ))
    .await
}

async fn watch_status_until<C, F>(
    args: &StatusArgs,
    client: &C,
    ctx: &OutputContext,
    interrupt: F,
) -> Result<()>
where
    C: StatusWatchClient,
    F: Future<Output = std::io::Result<()>>,
{
    let minimum_interval = Duration::from_secs_f64(args.interval.max(0.2));
    tokio::pin!(interrupt);
    let mut events = tokio::select! {
        subscription = client.subscribe_status_events() => {
            subscription.map_err(StatusWatchError::connection)?
        }
        signal = interrupt.as_mut() => {
            signal?;
            report_watch_stopped(ctx);
            return Ok(());
        }
    };
    let initial = tokio::select! {
        response = client.status_snapshot() => {
            response.map_err(StatusWatchError::connection)?
        }
        signal = interrupt.as_mut() => {
            signal?;
            events.close().await;
            report_watch_stopped(ctx);
            return Ok(());
        }
    };
    render_status(&initial, ctx)?;
    let mut last_rendered = tokio::time::Instant::now();

    loop {
        let next = tokio::select! {
            event = events.next_status_event() => {
                event.map_err(StatusWatchError::stream)?
            },
            signal = interrupt.as_mut() => {
                signal?;
                events.close().await;
                report_watch_stopped(ctx);
                return Ok(());
            }
        };
        if next.is_none() {
            return Err(StatusWatchError::stream(anyhow::anyhow!(
                "Daemon event stream closed while watching status"
            ))
            .into());
        }

        let deadline = last_rendered + minimum_interval;
        while tokio::time::Instant::now() < deadline {
            tokio::select! {
                () = tokio::time::sleep_until(deadline) => break,
                event = events.next_status_event() => {
                    if event.map_err(StatusWatchError::stream)?.is_none() {
                        return Err(StatusWatchError::stream(anyhow::anyhow!(
                            "Daemon event stream closed while watching status"
                        )).into());
                    }
                }
                signal = interrupt.as_mut() => {
                    signal?;
                    events.close().await;
                    report_watch_stopped(ctx);
                    return Ok(());
                }
            }
        }

        let status = tokio::select! {
            response = client.status_snapshot() => {
                response.map_err(StatusWatchError::stream)?
            }
            signal = interrupt.as_mut() => {
                signal?;
                events.close().await;
                report_watch_stopped(ctx);
                return Ok(());
            }
        };
        render_status(&status, ctx)?;
        last_rendered = tokio::time::Instant::now();
    }
}

fn report_watch_stopped(ctx: &OutputContext) {
    if !ctx.quiet {
        println!();
        ctx.info("Stopped status watch.");
    }
}

fn render_status(data: &SystemStatus, ctx: &OutputContext) -> Result<()> {
    match ctx.format {
        OutputFormat::Json => ctx.print_json(data)?,
        OutputFormat::Plain => {
            println!("{}", data.active_effect.as_deref().unwrap_or("Off"));
        }
        OutputFormat::Table => print_status_table(data, ctx),
    }
    Ok(())
}

// ── Rich table layout ──────────────────────────────────────────────────

fn print_status_table(data: &SystemStatus, ctx: &OutputContext) {
    if ctx.quiet {
        let running = data.running;
        let effect = data.active_effect.as_deref().unwrap_or("off");
        let dot = if ctx.painter.is_enabled() {
            ctx.painter.status_dot(running)
        } else if running {
            "*".to_string()
        } else {
            "x".to_string()
        };
        if let Some(scene) = format_scene_summary(data, &ctx.painter) {
            println!("  {dot} {effect}  {scene}");
        } else {
            println!("  {dot} {effect}");
        }
        return;
    }

    for line in status_table_lines(data, &ctx.painter) {
        println!("{line}");
    }
}

/// Build the rendered status lines as a Vec so tests can inspect them.
#[expect(
    clippy::too_many_lines,
    reason = "the status table is a single rich formatter with intentionally linear output order"
)]
fn status_table_lines(data: &SystemStatus, p: &Painter) -> Vec<String> {
    let mut lines = Vec::with_capacity(16);

    lines.push(String::new());

    lines.push(format!("  {}", p.help_banner_title()));
    lines.push(format!("  {}", p.muted(&"\u{2500}".repeat(21))));
    lines.push(String::new());

    // ── Header line: status · version · uptime ─────────────────────
    let running = data.running;
    let version = data.version.as_str();
    let uptime = data.uptime_seconds;

    let dot = if p.is_enabled() {
        p.status_dot(running)
    } else if running {
        "(*)".to_string()
    } else {
        "(x)".to_string()
    };
    let state_word = if running {
        p.success("running")
    } else {
        p.error("stopped")
    };
    lines.push(format!(
        "  {dot} {state_word}      {}  {}      {}  {}",
        p.muted("version"),
        p.number(version),
        p.muted("up"),
        p.number(&format_uptime(uptime)),
    ));
    lines.push(String::new());

    // ── Effect ──────────────────────────────────────────────────────
    if let Some(ownership) = &data.macos_daemon_ownership {
        let owner = humanize_macos_owner(ownership.active_owner.as_str());
        lines.push(format!(
            "  {}   {}  {}",
            p.muted(&pad("macOS owner", 10)),
            p.name(&owner),
            p.muted(&format!("epoch {}", ownership.owner_epoch)),
        ));

        if let Some(conflict) = &ownership.conflict {
            let contender = humanize_macos_owner(conflict.contender.as_str());
            lines.push(format!(
                "  {}   {}",
                p.muted(&pad("", 10)),
                p.warning(&format!("{contender} also attempted startup")),
            ));
        }

        if let Some(recovery) = &ownership.recovery_required {
            let phase = wire_label(&recovery.phase).replace('_', " ");
            lines.push(format!(
                "  {}   {}",
                p.muted(&pad("", 10)),
                p.warning(&format!("owner recovery required at {phase}")),
            ));
        }
        lines.push(String::new());
    }

    let effect_name = data.active_effect.as_deref().unwrap_or("off");
    lines.push(format!(
        "  {}   {}",
        p.muted(&pad("Effect", 10)),
        p.keyword(effect_name),
    ));
    if let Some(scene) = format_scene_summary(data, p) {
        lines.push(format!("  {}   {}", p.muted(&pad("Scene", 10)), scene));
    }

    // ── Render ──────────────────────────────────────────────────────
    let render_loop = &data.render_loop;
    let target_fps = render_loop.target_fps;
    let actual_fps = render_loop.actual_fps;
    let fps_tier = render_loop.fps_tier.as_str();
    let ceiling_fps = render_loop.ceiling_fps;
    let consecutive_misses = render_loop.consecutive_misses;
    let total_frames = render_loop.total_frames;

    let fps_ratio = if target_fps > 0 {
        (actual_fps / f64::from(target_fps)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let fps_pct = ratio_percent(fps_ratio);
    let bar = render_fps_bar(fps_ratio, 24, p);
    let fps_display = format!("{actual_fps:>4.1} {} {target_fps}", p.muted("/"));
    let health_word = format_fps_health(fps_pct, p);

    lines.push(format!(
        "  {}   {}  {bar}  {health_word}",
        p.muted(&pad("Render", 10)),
        fps_display,
    ));
    lines.push(format!(
        "  {}   {}  {}  {}  {}  {}",
        p.muted(&pad("", 10)),
        p.muted(&format!("{fps_tier} tier")),
        p.muted(&format!("ceiling {ceiling_fps}")),
        if consecutive_misses > 0 {
            p.error(&format!("{consecutive_misses} misses"))
        } else {
            p.muted("0 misses")
        },
        p.muted(&format!("{} frames", format_count(total_frames))),
        p.muted(""),
    ));

    // ── Frame budget ────────────────────────────────────────────────
    if let Some(latest_frame) = &data.latest_frame {
        let compositor_backend = latest_frame.compositor_backend.as_str();
        let total_ms = latest_frame.total_ms;
        let wake_late_ms = latest_frame.wake_late_ms;
        let frame_age_ms = latest_frame.frame_age_ms;
        let copy_count = latest_frame.full_frame_copy_count;
        let copy_kb = latest_frame.full_frame_copy_kb;
        let gpu_zone_sampling = latest_frame.gpu_zone_sampling;
        let cpu_readback_skipped = latest_frame.cpu_readback_skipped;

        lines.push(format!(
            "  {}   {} total  {} wake  {} age",
            p.muted(&pad("Frame", 10)),
            p.number(&format!("{total_ms:.2}ms")),
            p.number(&format!("{wake_late_ms:.2}ms")),
            p.number(&format!("{frame_age_ms:.2}ms")),
        ));
        lines.push(format!(
            "  {}   {} compose",
            p.muted(&pad("", 10)),
            p.keyword(&compositor_backend.replace('_', " ")),
        ));
        lines.push(format!(
            "  {}   {} gpu sample  {} readback",
            p.muted(&pad("", 10)),
            if gpu_zone_sampling {
                p.success("on")
            } else {
                p.muted("off")
            },
            if cpu_readback_skipped {
                p.success("skipped")
            } else {
                p.muted("materialized")
            },
        ));

        // ── Pipeline ────────────────────────────────────────────────
        let surfaces = &latest_frame.render_surfaces;
        lines.push(format!(
            "  {}   {} slots  {} free  {} published  {} dequeued",
            p.muted(&pad("Surfaces", 10)),
            p.number(&surfaces.scene_pool_slot_count.to_string()),
            p.number(&surfaces.scene_pool_free_slots.to_string()),
            p.number(&surfaces.scene_pool_published_slots.to_string()),
            p.number(&surfaces.scene_pool_dequeued_slots.to_string()),
        ));
        lines.push(format!(
            "  {}   {} copies ({})  {} canvas rx",
            p.muted(&pad("", 10)),
            p.number(&copy_count.to_string()),
            p.muted(&format_kib(copy_kb)),
            p.number(&surfaces.canvas_receivers.to_string()),
        ));
    }

    // ── Preview runtime ─────────────────────────────────────────────
    let preview = &data.preview_runtime;
    lines.push(format!(
        "  {}   {} rx ({} frames)  {} screen rx ({} frames)",
        p.muted(&pad("Preview", 10)),
        p.number(&preview.canvas_receivers.to_string()),
        p.muted(&format_count(preview.canvas_frames_published)),
        p.number(&preview.screen_canvas_receivers.to_string()),
        p.muted(&format_count(preview.screen_canvas_frames_published)),
    ));

    // ── Inventory ───────────────────────────────────────────────────
    let device_count = data.device_count;
    let effect_count = data.effect_count;
    lines.push(format!(
        "  {}   {} devices  {}  {} effects",
        p.muted(&pad("Inventory", 10)),
        p.number(&device_count.to_string()),
        p.muted("\u{00b7}"),
        p.number(&effect_count.to_string()),
    ));

    lines.push(String::new());

    lines
}

// ── Formatting helpers ─────────────────────────────────────────────────

fn pad(s: &str, width: usize) -> String {
    if s.chars().count() >= width {
        s.to_string()
    } else {
        format!("{s:<width$}")
    }
}

/// Format seconds as a human-readable uptime string.
fn format_uptime(seconds: u64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    }
    if seconds < 3_600 {
        let m = seconds / 60;
        let s = seconds % 60;
        return format!("{m}m {s}s");
    }
    if seconds < 86_400 {
        let h = seconds / 3_600;
        let m = (seconds % 3_600) / 60;
        return format!("{h}h {m}m");
    }
    let d = seconds / 86_400;
    let h = (seconds % 86_400) / 3_600;
    format!("{d}d {h}h")
}

/// Format a KiB value as KiB or MiB depending on magnitude.
fn format_kib(kib: f64) -> String {
    if kib >= 1024.0 {
        format!("{:.1} MiB", kib / 1024.0)
    } else {
        format!("{kib:.0} KiB")
    }
}

/// Format a large count with thousands separators.
fn format_count(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(char::from(b));
    }
    out
}

/// Render a progress bar for FPS ratio using cyan filled blocks and dim empty.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::as_conversions,
    reason = "ratio is clamped to 0..=1 and width is a bounded display width before rounding"
)]
fn render_fps_bar(ratio: f64, width: usize, p: &Painter) -> String {
    let filled = (ratio.clamp(0.0, 1.0) * (width as f64)).round() as usize;
    let filled = filled.min(width);
    let empty = width - filled;

    let filled_str = "\u{2588}".repeat(filled);
    let empty_str = "\u{2591}".repeat(empty);

    // Color the filled portion based on health:
    //   >=95% → cyan (healthy)
    //   >=80% → yellow (degraded)
    //   <80%  → red (critical)
    let pct = ratio * 100.0;
    let filled_colored = if pct >= 95.0 {
        p.name(&filled_str)
    } else if pct >= 80.0 {
        p.warning(&filled_str)
    } else {
        p.error(&filled_str)
    };

    format!("{filled_colored}{}", p.muted(&empty_str))
}

/// Percentage label with health coloring.
fn format_fps_health(pct: u32, p: &Painter) -> String {
    let text = format!("{pct}%");
    if pct >= 95 {
        p.success(&text)
    } else if pct >= 80 {
        p.warning(&text)
    } else {
        p.error(&text)
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions,
    reason = "ratio is clamped to 0..=1 before conversion to a 0..=100 display percentage"
)]
fn ratio_percent(ratio: f64) -> u32 {
    (ratio.clamp(0.0, 1.0) * 100.0).round() as u32
}

fn humanize_macos_owner(owner: &str) -> String {
    match owner {
        "app_sidecar" => "Hypercolor.app sidecar".to_owned(),
        "launchd_service" | "direct_launchd" => "launchd service".to_owned(),
        "homebrew_service" | "homebrew" => "Homebrew service".to_owned(),
        "standalone" => "terminal daemon".to_owned(),
        value => value.replace('_', " "),
    }
}

fn format_scene_summary(data: &SystemStatus, p: &Painter) -> Option<String> {
    let scene = data.active_scene.as_deref()?;
    let summary = p.name(scene);
    if data.active_scene_snapshot_locked {
        Some(format!("{summary} {}", p.warning("[snap]")))
    } else {
        Some(summary)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use anyhow::Result;
    use tokio::sync::{Mutex, Notify, mpsc, oneshot};

    use super::{
        StatusArgs, StatusWatchClient, StatusWatchError, StatusWatchEvents, exit_code_for_error,
        format_count, format_kib, format_uptime, status_from_system, status_table_lines,
        watch_status_until,
    };
    use crate::output::{OutputContext, OutputFormat, Painter};
    use hypercolor_types::api::system::{
        LatestFrameStatus, MacosCapabilityOwner, MacosDaemonHandoverPhase,
        MacosDaemonOwnerConflictStatus, MacosDaemonOwnerRecoveryRequiredStatus,
        MacosDaemonOwnershipStatus, PreviewRuntimeStatus, RenderLoopStatus, RenderSurfaceStatus,
        ServerInfo, SystemResource, SystemStatus,
    };
    use serde_json::json;

    fn status_with_effect(effect: &str) -> SystemStatus {
        SystemStatus {
            active_effect: Some(effect.to_owned()),
            ..SystemStatus::default()
        }
    }

    struct FakeWatchClient {
        statuses: Mutex<mpsc::UnboundedReceiver<Result<SystemStatus>>>,
        events: Mutex<Option<mpsc::UnboundedReceiver<Result<Option<serde_json::Value>>>>>,
        subscriptions: AtomicUsize,
        status_requests: AtomicUsize,
        closed: Arc<AtomicBool>,
        subscription_gate: Option<Arc<Notify>>,
    }

    struct FakeWatchEvents {
        events: mpsc::UnboundedReceiver<Result<Option<serde_json::Value>>>,
        closed: Arc<AtomicBool>,
    }

    type FakeStatusSender = mpsc::UnboundedSender<Result<SystemStatus>>;
    type FakeEventSender = mpsc::UnboundedSender<Result<Option<serde_json::Value>>>;

    impl StatusWatchClient for FakeWatchClient {
        type Events = FakeWatchEvents;

        async fn subscribe_status_events(&self) -> Result<Self::Events> {
            self.subscriptions.fetch_add(1, Ordering::AcqRel);
            if let Some(gate) = &self.subscription_gate {
                gate.notified().await;
            }
            let events = self
                .events
                .lock()
                .await
                .take()
                .ok_or_else(|| anyhow::anyhow!("fixture subscription already consumed"))?;
            Ok(FakeWatchEvents {
                events,
                closed: Arc::clone(&self.closed),
            })
        }

        async fn status_snapshot(&self) -> Result<SystemStatus> {
            self.status_requests.fetch_add(1, Ordering::AcqRel);
            self.statuses
                .lock()
                .await
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("fixture status stream closed"))?
        }
    }

    impl StatusWatchEvents for FakeWatchEvents {
        async fn next_status_event(&mut self) -> Result<Option<serde_json::Value>> {
            self.events.recv().await.unwrap_or(Ok(None))
        }

        async fn close(self) {
            self.closed.store(true, Ordering::Release);
        }
    }

    fn fake_watch_client() -> (Arc<FakeWatchClient>, FakeStatusSender, FakeEventSender) {
        let (status_tx, status_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        (
            Arc::new(FakeWatchClient {
                statuses: Mutex::new(status_rx),
                events: Mutex::new(Some(event_rx)),
                subscriptions: AtomicUsize::new(0),
                status_requests: AtomicUsize::new(0),
                closed: Arc::new(AtomicBool::new(false)),
                subscription_gate: None,
            }),
            status_tx,
            event_tx,
        )
    }

    fn watch_context() -> OutputContext {
        OutputContext::new(OutputFormat::Plain, false, true, true, None)
    }

    #[test]
    fn system_projection_extracts_authenticated_status() {
        let status = status_from_system(SystemResource {
            identity: ServerInfo {
                instance_id: "server".to_owned(),
                ..ServerInfo::default()
            },
            status: Some(SystemStatus {
                running: true,
                ..SystemStatus::default()
            }),
        })
        .expect("authenticated status should be present");

        assert!(status.running);
    }

    #[test]
    fn system_projection_rejects_missing_status() {
        let error = status_from_system(SystemResource {
            identity: ServerInfo {
                instance_id: "server".to_owned(),
                ..ServerInfo::default()
            },
            status: None,
        })
        .expect_err("status should require read access");
        assert!(error.to_string().contains("requires daemon read access"));
    }

    async fn wait_for_status_requests(client: &FakeWatchClient, expected: usize) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while client.status_requests.load(Ordering::Acquire) != expected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("fixture should reach the expected request count");
    }

    async fn wait_for_subscriptions(client: &FakeWatchClient, expected: usize) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while client.subscriptions.load(Ordering::Acquire) != expected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("fixture should reach the expected subscription count");
    }

    #[tokio::test]
    async fn watch_interrupt_cancels_a_blocked_subscription() {
        let (status_tx, status_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let client = Arc::new(FakeWatchClient {
            statuses: Mutex::new(status_rx),
            events: Mutex::new(Some(event_rx)),
            subscriptions: AtomicUsize::new(0),
            status_requests: AtomicUsize::new(0),
            closed: Arc::new(AtomicBool::new(false)),
            subscription_gate: Some(Arc::new(Notify::new())),
        });
        let (interrupt_tx, interrupt_rx) = oneshot::channel();
        let task = tokio::spawn({
            let client = Arc::clone(&client);
            async move {
                watch_status_until(
                    &StatusArgs {
                        watch: true,
                        interval: 0.2,
                    },
                    client.as_ref(),
                    &watch_context(),
                    async move {
                        interrupt_rx
                            .await
                            .map_err(|_| std::io::Error::other("fixture interrupt sender dropped"))
                    },
                )
                .await
            }
        });

        wait_for_subscriptions(&client, 1).await;
        interrupt_tx.send(()).expect("interrupt should deliver");
        task.await
            .expect("watch task should join")
            .expect("interrupt should cancel the blocked subscription");

        assert_eq!(client.status_requests.load(Ordering::Acquire), 0);
        assert!(!client.closed.load(Ordering::Acquire));
        drop(status_tx);
        drop(event_tx);
    }

    #[tokio::test]
    async fn watch_refreshes_only_after_events_and_reports_stream_close() {
        let (client, status_tx, event_tx) = fake_watch_client();
        status_tx
            .send(Ok(status_with_effect("initial")))
            .expect("initial status should queue");
        let task = tokio::spawn({
            let client = Arc::clone(&client);
            async move {
                watch_status_until(
                    &StatusArgs {
                        watch: true,
                        interval: 0.2,
                    },
                    client.as_ref(),
                    &watch_context(),
                    std::future::pending::<std::io::Result<()>>(),
                )
                .await
            }
        });

        wait_for_status_requests(&client, 1).await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(client.status_requests.load(Ordering::Acquire), 1);

        status_tx
            .send(Ok(status_with_effect("updated")))
            .expect("updated status should queue");
        event_tx
            .send(Ok(Some(json!({ "type": "event" }))))
            .expect("event should queue");
        wait_for_status_requests(&client, 2).await;
        drop(event_tx);

        let error = task
            .await
            .expect("watch task should join")
            .expect_err("unexpected stream close should fail");
        assert_eq!(exit_code_for_error(&error), Some(1));
        assert_eq!(client.subscriptions.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn watch_coalesces_event_bursts_and_closes_on_interrupt() {
        let (client, status_tx, event_tx) = fake_watch_client();
        status_tx
            .send(Ok(status_with_effect("initial")))
            .expect("initial status should queue");
        status_tx
            .send(Ok(status_with_effect("coalesced")))
            .expect("coalesced status should queue");
        let (interrupt_tx, interrupt_rx) = oneshot::channel();
        let task = tokio::spawn({
            let client = Arc::clone(&client);
            async move {
                watch_status_until(
                    &StatusArgs {
                        watch: true,
                        interval: 0.2,
                    },
                    client.as_ref(),
                    &watch_context(),
                    async move {
                        interrupt_rx
                            .await
                            .map_err(|_| std::io::Error::other("fixture interrupt sender dropped"))
                    },
                )
                .await
            }
        });

        wait_for_status_requests(&client, 1).await;
        for sequence in 1..=3 {
            event_tx
                .send(Ok(Some(json!({ "sequence": sequence }))))
                .expect("burst event should queue");
        }
        wait_for_status_requests(&client, 2).await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(client.status_requests.load(Ordering::Acquire), 2);

        interrupt_tx.send(()).expect("interrupt should deliver");
        task.await
            .expect("watch task should join")
            .expect("interrupt should stop cleanly");
        assert!(client.closed.load(Ordering::Acquire));
        assert_eq!(client.subscriptions.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn watch_interrupt_remains_live_during_rest_refresh() {
        let (client, status_tx, event_tx) = fake_watch_client();
        status_tx
            .send(Ok(status_with_effect("initial")))
            .expect("initial status should queue");
        let (interrupt_tx, interrupt_rx) = oneshot::channel();
        let task = tokio::spawn({
            let client = Arc::clone(&client);
            async move {
                watch_status_until(
                    &StatusArgs {
                        watch: true,
                        interval: 0.2,
                    },
                    client.as_ref(),
                    &watch_context(),
                    async move {
                        interrupt_rx
                            .await
                            .map_err(|_| std::io::Error::other("fixture interrupt sender dropped"))
                    },
                )
                .await
            }
        });

        wait_for_status_requests(&client, 1).await;
        tokio::time::sleep(Duration::from_millis(220)).await;
        event_tx
            .send(Ok(Some(json!({ "type": "event" }))))
            .expect("event should queue");
        wait_for_status_requests(&client, 2).await;
        interrupt_tx.send(()).expect("interrupt should deliver");

        task.await
            .expect("watch task should join")
            .expect("interrupt should cancel an in-flight refresh");
        assert!(client.closed.load(Ordering::Acquire));
    }

    #[test]
    fn watch_connection_failures_use_exit_code_two() {
        let error: anyhow::Error = StatusWatchError::connection(anyhow::anyhow!("offline")).into();
        assert_eq!(exit_code_for_error(&error), Some(2));
    }

    #[test]
    fn format_uptime_formats_correctly() {
        assert_eq!(format_uptime(0), "0s");
        assert_eq!(format_uptime(45), "45s");
        assert_eq!(format_uptime(75), "1m 15s");
        assert_eq!(format_uptime(3_661), "1h 1m");
        assert_eq!(format_uptime(90_000), "1d 1h");
    }

    #[test]
    fn format_kib_switches_to_mib() {
        assert_eq!(format_kib(512.0), "512 KiB");
        assert_eq!(format_kib(2_048.0), "2.0 MiB");
    }

    #[test]
    fn format_count_adds_thousands_separators() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_234), "1,234");
        assert_eq!(format_count(1_234_567), "1,234,567");
    }

    #[test]
    fn status_lines_include_core_fields() {
        let data = SystemStatus {
            running: true,
            version: "1.0.0".to_owned(),
            uptime_seconds: 3_661,
            render_loop: RenderLoopStatus {
                fps_tier: "60fps".to_owned(),
                target_fps: 60,
                ceiling_fps: 60,
                actual_fps: 59.8,
                consecutive_misses: 0,
                total_frames: 1_234,
                ..RenderLoopStatus::default()
            },
            active_effect: Some("Breakthrough".to_owned()),
            active_scene: Some("Movie Night".to_owned()),
            active_scene_snapshot_locked: true,
            device_count: 5,
            effect_count: 18,
            macos_daemon_ownership: Some(MacosDaemonOwnershipStatus {
                active_owner: MacosCapabilityOwner::LaunchdService,
                owner_epoch: 7,
                conflict: Some(MacosDaemonOwnerConflictStatus {
                    active: MacosCapabilityOwner::LaunchdService,
                    contender: MacosCapabilityOwner::HomebrewService,
                    observed_at_ms: 42,
                }),
                recovery_required: Some(MacosDaemonOwnerRecoveryRequiredStatus {
                    requested_owner: MacosCapabilityOwner::HomebrewService,
                    prior_owner: MacosCapabilityOwner::LaunchdService,
                    phase: MacosDaemonHandoverPhase::RequestedOwnerStarted,
                }),
            }),
            latest_frame: Some(LatestFrameStatus {
                frame_token: 77,
                compositor_backend: "gpu_fallback".to_owned(),
                gpu_zone_sampling: true,
                cpu_readback_skipped: true,
                total_ms: 4.32,
                wake_late_ms: 0.15,
                frame_age_ms: 8.5,
                full_frame_copy_count: 1,
                full_frame_copy_kb: 250.0,
                render_surfaces: RenderSurfaceStatus {
                    scene_pool_slot_count: 6,
                    scene_pool_free_slots: 0,
                    scene_pool_published_slots: 4,
                    scene_pool_dequeued_slots: 2,
                    canvas_receivers: 2,
                    ..RenderSurfaceStatus::default()
                },
                ..LatestFrameStatus::default()
            }),
            preview_runtime: PreviewRuntimeStatus {
                canvas_receivers: 1,
                screen_canvas_receivers: 0,
                canvas_frames_published: 88,
                screen_canvas_frames_published: 12,
                ..PreviewRuntimeStatus::default()
            },
            ..SystemStatus::default()
        };

        let painter = Painter::plain();
        let lines = status_table_lines(&data, &painter);
        let joined = lines.join("\n");
        let running_index = lines
            .iter()
            .position(|line| line.contains("running"))
            .expect("daemon state should render");
        let owner_index = lines
            .iter()
            .position(|line| line.contains("macOS owner"))
            .expect("macOS owner should render");

        assert!(joined.contains("Breakthrough"), "effect name present");
        assert!(joined.contains("Movie Night"), "scene name present");
        assert!(joined.contains("[snap]"), "snapshot marker present");
        assert!(joined.contains("running"), "running state present");
        assert!(joined.contains("1.0.0"), "version present");
        assert!(joined.contains("1h 1m"), "uptime formatted");
        assert!(joined.contains("59.8"), "actual fps present");
        assert!(joined.contains("60fps tier"), "tier present");
        assert!(
            joined.contains("1,234 frames"),
            "frame count with separator"
        );
        assert!(joined.contains("6 slots"), "surface count present");
        assert!(joined.contains("250 KiB"), "copy size present");
        assert!(
            joined.contains("gpu fallback compose"),
            "backend mode present"
        );
        assert!(
            joined.contains("on gpu sample  skipped readback"),
            "gpu sampling telemetry present"
        );
        assert!(joined.contains("5 devices"), "device count present");
        assert!(joined.contains("18 effects"), "effect count present");
        assert!(joined.contains("launchd service"), "owner name present");
        assert!(joined.contains("epoch 7"), "owner epoch present");
        assert!(
            owner_index > running_index,
            "ownership should follow the daemon header"
        );
        assert!(
            joined.contains("Homebrew service also attempted startup"),
            "owner conflict present"
        );
        assert!(
            joined.contains("owner recovery required at requested owner started"),
            "owner recovery present"
        );
    }
}
