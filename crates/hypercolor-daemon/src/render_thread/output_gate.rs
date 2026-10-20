//! Output-demand gate: park the render loop while nothing consumes frames.
//!
//! Every frame costs producer, compositor, and GPU work, so a loop that
//! renders with no device, display, preview, or capture attached spends a
//! fixed per-frame cost on output no one receives. The gate removes that
//! cost without changing any observable cadence: before each frame it
//! counts consumers (see [`crate::output_demand`]), and with none it parks
//! the loop instead of rendering. A parked loop sleeps on the bus output
//! demand signal, the device registry, the input demand registry, and the
//! scene transaction queue, so the first consumer to appear wakes it at
//! once and it resumes at its full tier. A long safety tick re-evaluates in
//! case some consumer arrived without signalling.
//!
//! Parking waits for the producers to settle first. After the scene, the
//! effect catalog, or the canvas changes, the loop keeps rendering until
//! every producer is warm and any scene transition has finished, so an
//! effect applied while nobody watches is loaded and showing its own
//! content by the time a consumer arrives.

use std::time::{Duration, Instant};

use hypercolor_core::input::SourceKind;
use tokio::pin;

use super::RenderThreadState;
use super::input_publication::InputPublicationDemandHandle;
use super::pipeline_runtime::FrameLoopState;
use crate::output_demand::{OutputConsumers, is_output_backend, output_device_count};

/// How often a parked loop re-evaluates demand without any wake signal.
pub(crate) const PARKED_SAFETY_TICK: Duration = Duration::from_secs(1);

/// Consecutive frame starts with no producer warming before the loop may
/// park. The first frame after a change reconciles the producers, so one
/// clean start alone does not prove a new producer is warm.
const SETTLE_WARM_FRAMES: u32 = 3;

/// Longest the loop keeps rendering for producers that never finish
/// warming, such as a page that never loads. Matches the Servo worker's
/// ready timeout, the longest a session creation or page load may block.
const SETTLE_LIMIT: Duration = Duration::from_secs(10);

/// Everything the producers render from, so a change to any of it is work
/// the loop must render before it parks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProducerSignature {
    scene_generation: u64,
    zones_revision: u64,
    effect_registry_generation: u64,
    zone_layout_preview_generation: u64,
    canvas_width: u32,
    canvas_height: u32,
}

impl ProducerSignature {
    pub(crate) async fn observe(state: &RenderThreadState) -> Self {
        let (scene_generation, zones_revision) = {
            let plan = state.scene_plan.load();
            (plan.generation, plan.zones_revision)
        };
        let effect_registry_generation = state.effect_registry.read().await.generation();
        Self {
            scene_generation,
            zones_revision,
            effect_registry_generation,
            zone_layout_preview_generation: state.zone_layout_previews.generation(),
            canvas_width: state.canvas_dims.width(),
            canvas_height: state.canvas_dims.height(),
        }
    }
}

/// Whether the next frame renders or the loop parks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GateDecision {
    Render,
    Park,
}

/// Why a parked loop woke.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderWakeReason {
    /// A device, stream receiver, or demand hold appeared.
    OutputConsumer,
    /// A queued scene transaction has a caller waiting on it.
    SceneTransaction,
    /// The scene, effect catalog, or canvas changed, so the producers must
    /// settle on the new state.
    ProducerState,
    /// The loop was paused or stopped, which the main loop handles.
    RenderLoopState,
}

impl RenderWakeReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::OutputConsumer => "output_consumer",
            Self::SceneTransaction => "scene_transaction",
            Self::ProducerState => "producer_state",
            Self::RenderLoopState => "render_loop_state",
        }
    }
}

/// Run of frame starts under one producer signature.
#[derive(Clone, Copy, Debug)]
struct WarmStreak {
    signature: ProducerSignature,
    warm_frames: u32,
    since: Instant,
}

/// Render-thread state for the output-demand gate.
#[derive(Debug, Default)]
pub(crate) struct OutputGate {
    /// Registry output device count keyed by the registry generation it
    /// was read at.
    registry_devices: Option<(u64, u32)>,
    /// Consumers last recorded into render telemetry.
    recorded: Option<OutputConsumers>,
    streak: Option<WarmStreak>,
    /// Whether the last rendered frame was mid scene transition.
    transition_active: bool,
    /// Whether the loop is parked, from the park until a frame renders or
    /// the loop leaves the running state.
    parked: bool,
}

impl OutputGate {
    /// Count consumers and record them for status surfaces when they
    /// changed.
    ///
    /// The device registry is read only when its generation moved. When
    /// neither a registered device nor any bus-side consumer exists, the
    /// backend manager's routes are counted too, so a device mapped for
    /// output counts even before the registry reports it renderable.
    pub(crate) async fn consumers(
        &mut self,
        state: &RenderThreadState,
        input_demands: &InputPublicationDemandHandle,
    ) -> OutputConsumers {
        let generation = state.device_registry.generation();
        let registry_devices = match self.registry_devices {
            Some((counted_at, count)) if counted_at == generation => count,
            _ => {
                // A mutation racing the count leaves the cache keyed to the
                // older generation, so the next evaluation counts again.
                let count = output_device_count(&state.device_registry).await;
                self.registry_devices = Some((generation, count));
                count
            }
        };
        let input_event_streams =
            u32::try_from(input_demands.stream_registrations_requesting(SourceKind::Interaction))
                .unwrap_or(u32::MAX);
        let mut consumers =
            OutputConsumers::from_bus(&state.event_bus, registry_devices, input_event_streams);
        if !consumers.any() {
            consumers.output_devices = routed_output_device_count(state).await;
        }
        if self.recorded != Some(consumers) {
            self.recorded = Some(consumers);
            state
                .performance
                .write()
                .await
                .note_output_consumers(consumers);
        }
        consumers
    }

    /// Record whether the frame just rendered was mid scene transition.
    ///
    /// A transition advances only as frames render, so the loop keeps
    /// rendering until it completes rather than freezing it mid crossfade
    /// with the outgoing scene's producers still alive. Only frames that
    /// reach composition update the flag, and the settle limit still
    /// bounds the wait, so a crossfade longer than that limit pauses
    /// partway and finishes once a consumer returns.
    pub(crate) fn note_scene_transition(&mut self, active: bool) {
        self.transition_active = active;
    }

    /// Whether the previous rendered frame was mid scene transition.
    pub(crate) const fn transition_active(&self) -> bool {
        self.transition_active
    }

    /// Record that the loop parked.
    pub(crate) fn enter_park(&mut self) {
        self.parked = true;
    }

    /// Record that the loop left its park, returning whether it was
    /// parked.
    pub(crate) fn leave_park(&mut self) -> bool {
        std::mem::take(&mut self.parked)
    }

    /// Decide whether this frame renders.
    ///
    /// `producers_warming` describes the producers as the previous frame
    /// left them. Frames that consumers drive count toward settling too, so
    /// losing the last consumer parks at once when the producers are warm.
    pub(crate) fn decide(
        &mut self,
        consumed: bool,
        signature: ProducerSignature,
        producers_warming: bool,
        now: Instant,
    ) -> GateDecision {
        let streak = match &mut self.streak {
            Some(streak) if streak.signature == signature => streak,
            _ => self.streak.insert(WarmStreak {
                signature,
                warm_frames: 0,
                since: now,
            }),
        };
        streak.warm_frames = if producers_warming {
            0
        } else {
            streak.warm_frames.saturating_add(1)
        };
        if consumed {
            return GateDecision::Render;
        }
        let settled = streak.warm_frames >= SETTLE_WARM_FRAMES
            || now.saturating_duration_since(streak.since) >= SETTLE_LIMIT;
        if settled {
            GateDecision::Park
        } else {
            GateDecision::Render
        }
    }

    /// Why a parked loop should resume now, if it should.
    pub(crate) async fn resume_reason(
        &mut self,
        state: &RenderThreadState,
        input_demands: &InputPublicationDemandHandle,
    ) -> Option<RenderWakeReason> {
        if !state.render_loop.read().await.is_running() {
            return Some(RenderWakeReason::RenderLoopState);
        }
        if state.scene_transactions.has_pending() {
            return Some(RenderWakeReason::SceneTransaction);
        }
        if self.consumers(state, input_demands).await.any() {
            return Some(RenderWakeReason::OutputConsumer);
        }
        let signature = ProducerSignature::observe(state).await;
        if self
            .streak
            .is_none_or(|streak| streak.signature != signature)
        {
            return Some(RenderWakeReason::ProducerState);
        }
        None
    }
}

/// Count devices the backend manager routes frames to, simulated displays
/// excluded, each counted once however many layout ids bind it.
///
/// Rigs carry a handful of routes, so the quadratic dedupe stays cheaper
/// than allocating a set on every frame.
async fn routed_output_device_count(state: &RenderThreadState) -> u32 {
    let manager = state.backend_manager.lock().await;
    let routed = || {
        manager
            .mapped_device_targets()
            .filter(|(backend_id, _)| is_output_backend(backend_id))
            .map(|(_, device_id)| device_id)
    };
    let count = routed()
        .enumerate()
        .filter(|(index, device_id)| !routed().take(*index).any(|seen| seen == *device_id))
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// Sleep until a parked loop has a reason to run a frame.
///
/// Each signal is armed before demand is re-evaluated, so a consumer that
/// appears between the evaluation and the sleep still wakes the loop.
pub(crate) async fn wait_for_output_demand(
    state: &RenderThreadState,
    frame_loop: &mut FrameLoopState,
) -> RenderWakeReason {
    let armed_output_demand = state.event_bus.output_demand().arm();
    loop {
        let output_demand = armed_output_demand.notified();
        let devices = state.device_registry.changed();
        let transactions = state.scene_transactions.queued();
        let mut input_demand_revisions = frame_loop.input_demands.subscribe_revision();
        pin!(output_demand, devices, transactions);
        output_demand.as_mut().enable();
        devices.as_mut().enable();
        transactions.as_mut().enable();

        if let Some(reason) = frame_loop
            .output_gate
            .resume_reason(state, &frame_loop.input_demands)
            .await
        {
            return reason;
        }

        tokio::select! {
            () = &mut output_demand => {}
            () = &mut devices => {}
            () = &mut transactions => {}
            _ = input_demand_revisions.changed() => {}
            () = tokio::time::sleep(PARKED_SAFETY_TICK) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{GateDecision, OutputGate, ProducerSignature, SETTLE_LIMIT};

    fn signature(scene_generation: u64) -> ProducerSignature {
        ProducerSignature {
            scene_generation,
            zones_revision: 1,
            effect_registry_generation: 1,
            zone_layout_preview_generation: 0,
            canvas_width: 640,
            canvas_height: 480,
        }
    }

    #[test]
    fn unconsumed_loop_renders_until_producers_settle_then_parks() {
        let mut gate = OutputGate::default();
        let now = Instant::now();

        assert_eq!(
            gate.decide(false, signature(1), false, now),
            GateDecision::Render
        );
        assert_eq!(
            gate.decide(false, signature(1), false, now),
            GateDecision::Render
        );
        assert_eq!(
            gate.decide(false, signature(1), false, now),
            GateDecision::Park
        );
        assert_eq!(
            gate.decide(false, signature(1), false, now),
            GateDecision::Park
        );
    }

    #[test]
    fn warming_producer_holds_the_loop_awake_until_it_warms() {
        let mut gate = OutputGate::default();
        let now = Instant::now();

        for _ in 0..8 {
            assert_eq!(
                gate.decide(false, signature(1), true, now),
                GateDecision::Render
            );
        }
        assert_eq!(
            gate.decide(false, signature(1), false, now),
            GateDecision::Render
        );
        assert_eq!(
            gate.decide(false, signature(1), false, now),
            GateDecision::Render
        );
        assert_eq!(
            gate.decide(false, signature(1), false, now),
            GateDecision::Park
        );
    }

    #[test]
    fn producer_that_never_warms_parks_after_the_settle_limit() {
        let mut gate = OutputGate::default();
        let start = Instant::now();

        assert_eq!(
            gate.decide(false, signature(1), true, start),
            GateDecision::Render
        );
        let late = start + SETTLE_LIMIT + Duration::from_millis(1);
        assert_eq!(
            gate.decide(false, signature(1), true, late),
            GateDecision::Park
        );
    }

    #[test]
    fn consumers_always_render_and_count_toward_settling() {
        let mut gate = OutputGate::default();
        let now = Instant::now();

        for _ in 0..4 {
            assert_eq!(
                gate.decide(true, signature(1), false, now),
                GateDecision::Render
            );
        }
        assert_eq!(
            gate.decide(false, signature(1), false, now),
            GateDecision::Park,
            "producers already warm under consumers should park as soon as the last consumer leaves"
        );
    }

    #[test]
    fn producer_change_while_unconsumed_settles_again() {
        let mut gate = OutputGate::default();
        let now = Instant::now();
        for _ in 0..3 {
            let _ = gate.decide(false, signature(1), false, now);
        }
        assert_eq!(
            gate.decide(false, signature(1), false, now),
            GateDecision::Park
        );

        assert_eq!(
            gate.decide(false, signature(2), false, now),
            GateDecision::Render
        );
        assert_eq!(
            gate.decide(false, signature(2), false, now),
            GateDecision::Render
        );
        assert_eq!(
            gate.decide(false, signature(2), false, now),
            GateDecision::Park
        );
    }
}
