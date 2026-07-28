//! Deterministic Windows input and capture integration fixtures.
//!
//! These tests stop at the platform boundary: Raw Input reports and desktop
//! pixels are injected in memory, then production folding, lifecycle, graph
//! publication, and render routing consume them without touching host state.

#![cfg(target_os = "windows")]

use std::sync::{Arc, Mutex};

use hypercolor_core::input::routing::{
    ConsumerIncarnation, InteractionRouteContext, InteractionRouteRequest, InteractionRouteSource,
    InteractionRouter, RoutedInteraction,
};
use hypercolor_core::input::screen::{CaptureConfig, ScreenCaptureInput};
use hypercolor_core::input::{
    InputData, InputManager, InputSource, InteractionData, SourceFreshness, SourceKind,
    SourceState, SourceStatusHandle, SourceStatusReporter, WindowsHostInput,
};
use hypercolor_types::event::{InputButtonState, InputEvent, TimedInputEvent};
use hypercolor_windows_input::{
    RawButton, RawCursor, RawDeviceDescriptor, RawDeviceKind, RawInputBatch, RawInputEvent,
    RawKeyPrefix,
};

#[derive(Clone)]
struct RawInputFixtureHandle {
    pending: Arc<Mutex<Option<RawInputFixtureBatch>>>,
}

struct RawInputFixtureBatch {
    events: Vec<RawInputEvent>,
    cursor: Option<RawCursor>,
    at_ms: u64,
}

impl RawInputFixtureHandle {
    fn inject(&self, events: Vec<RawInputEvent>, cursor: Option<RawCursor>, at_ms: u64) {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            pending.is_none(),
            "fixture batch must be consumed before reinjection"
        );
        *pending = Some(RawInputFixtureBatch {
            events,
            cursor,
            at_ms,
        });
    }
}

struct RawInputFixtureSource {
    fold: WindowsHostInput,
    pending: Arc<Mutex<Option<RawInputFixtureBatch>>>,
    latest: InteractionData,
    running: bool,
    status: SourceStatusReporter,
}

impl RawInputFixtureSource {
    fn new() -> (Self, RawInputFixtureHandle) {
        let pending = Arc::new(Mutex::new(None));
        (
            Self {
                fold: WindowsHostInput::new(true, true),
                pending: Arc::clone(&pending),
                latest: InteractionData::default(),
                running: false,
                status: SourceStatusReporter::new(
                    "windows_raw_input_fixture",
                    SourceKind::Interaction,
                    "raw_input",
                    true,
                    true,
                    true,
                ),
            },
            RawInputFixtureHandle { pending },
        )
    }

    fn sample_fixture(&mut self) -> (InputData, Vec<TimedInputEvent>) {
        let batch = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let Some(batch) = batch else {
            return (InputData::Interaction(self.latest.clone()), Vec::new());
        };
        let (snapshot, events) = self.fold.fold_and_snapshot(RawInputBatch {
            events: &batch.events,
            cursor: batch.cursor,
            at_ms: batch.at_ms,
            epoch: self.fold.epoch(),
        });
        self.latest.clone_from(&snapshot);
        (InputData::Interaction(snapshot), events)
    }
}

impl InputSource for RawInputFixtureSource {
    fn name(&self) -> &'static str {
        "windows_raw_input_fixture"
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.running {
            return Ok(());
        }
        if let Some(session) = self.status.begin_session()? {
            assert!(session.mark_event_driven_live_without_deadline(2));
        }
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        self.status.stop();
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(self.sample_fixture().0)
    }

    fn sample_and_drain_with_delta_secs(
        &mut self,
        _delta_secs: f32,
    ) -> (anyhow::Result<InputData>, Vec<TimedInputEvent>) {
        let (sample, events) = self.sample_fixture();
        (Ok(sample), events)
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_interaction_source(&self) -> bool {
        true
    }

    fn is_host_capture_source(&self) -> bool {
        true
    }
}

#[derive(Clone)]
struct ScreenCaptureFixtureHandle {
    pending: Arc<Mutex<Option<ScreenCaptureFixtureFrame>>>,
}

struct ScreenCaptureFixtureFrame {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl ScreenCaptureFixtureHandle {
    fn inject(&self, rgba: Vec<u8>, width: u32, height: u32) {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            pending.is_none(),
            "fixture frame must be consumed before reinjection"
        );
        *pending = Some(ScreenCaptureFixtureFrame {
            rgba,
            width,
            height,
        });
    }
}

struct ScreenCaptureFixtureSource {
    source: ScreenCaptureInput,
    pending: Arc<Mutex<Option<ScreenCaptureFixtureFrame>>>,
}

impl ScreenCaptureFixtureSource {
    fn new(config: CaptureConfig) -> (Self, ScreenCaptureFixtureHandle) {
        let pending = Arc::new(Mutex::new(None));
        (
            Self {
                source: ScreenCaptureInput::new(config),
                pending: Arc::clone(&pending),
            },
            ScreenCaptureFixtureHandle { pending },
        )
    }
}

impl InputSource for ScreenCaptureFixtureSource {
    fn name(&self) -> &str {
        self.source.name()
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        self.source.source_status_handle()
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        self.source.source_status_reporter()
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.source.start()
    }

    fn stop(&mut self) {
        self.source.stop();
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        if let Some(frame) = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            self.source
                .push_frame(&frame.rgba, frame.width, frame.height);
        }
        self.source.sample()
    }

    fn is_running(&self) -> bool {
        self.source.is_running()
    }

    fn is_screen_source(&self) -> bool {
        true
    }
}

fn raw_device(source_id: &'static str, kind: RawDeviceKind) -> Arc<RawDeviceDescriptor> {
    Arc::new(RawDeviceDescriptor {
        source_id: Arc::from(source_id),
        interface_path: Some(Arc::from(format!("fixture:{source_id}"))),
        label: Arc::from(format!("fixture {source_id}")),
        kind,
        session_generation: 1,
        device_generation: 1,
    })
}

#[test]
fn raw_input_flows_through_health_graph_and_render_route() {
    let keyboard = raw_device("keyboard-1", RawDeviceKind::Keyboard);
    let mouse = raw_device("mouse-1", RawDeviceKind::Mouse);
    let events = vec![
        RawInputEvent::DeviceArrived {
            device: Arc::clone(&keyboard),
        },
        RawInputEvent::Key {
            device: Arc::clone(&keyboard),
            make_code: 0x1e,
            prefix: RawKeyPrefix::None,
            vkey: 0,
            pressed: true,
        },
        RawInputEvent::Key {
            device: keyboard,
            make_code: 0x1e,
            prefix: RawKeyPrefix::None,
            vkey: 0,
            pressed: true,
        },
        RawInputEvent::DeviceArrived {
            device: Arc::clone(&mouse),
        },
        RawInputEvent::Button {
            device: Arc::clone(&mouse),
            button: RawButton::Left,
            pressed: true,
        },
        RawInputEvent::MotionRelative {
            device: mouse,
            dx: 120,
            dy: -60,
        },
    ];
    let (source, fixture) = RawInputFixtureSource::new();
    let mut manager = InputManager::new();
    let graph = manager.input_graph_handle();
    let statuses = manager.source_status_registry();
    manager.add_source(Box::new(source));
    manager.start_all().expect("fixture source starts");

    let initial_graph = graph.snapshot();
    let route_source = InteractionRouteSource::manager_slot(
        "raw_input",
        initial_graph.slots()[0]
            .status()
            .snapshot()
            .session_generation,
        initial_graph.slots()[0].clone(),
    )
    .expect("interaction slot is routable");
    let consumer = ConsumerIncarnation::new(1);
    let mut routed = RoutedInteraction::new(consumer);
    let mut router = InteractionRouter::default();
    router.resolve_into(
        consumer,
        InteractionRouteRequest::host(),
        std::slice::from_ref(&route_source),
        InteractionRouteContext {
            source_graph_generation: initial_graph.generation(),
            now_ms: 999,
            ..InteractionRouteContext::default()
        },
        &mut routed,
    );
    assert!(routed.interaction.keyboard.pressed_keys.is_empty());

    fixture.inject(
        events,
        Some(RawCursor {
            x: -100,
            y: 200,
            norm_x: 0.25,
            norm_y: 0.75,
        }),
        1_000,
    );
    manager.sample_sources(1.0 / 60.0);

    let graph = graph.snapshot();
    assert_eq!(graph.slots().len(), 1);
    assert_eq!(graph.slots()[0].kind(), Some(SourceKind::Interaction));
    let mut published_events = Vec::new();
    let publication = graph.slots()[0].read_publication_since(0, &mut published_events);
    let interaction = match publication.sample.as_deref() {
        Some(InputData::Interaction(interaction)) => interaction,
        other => panic!("expected interaction publication, got {other:?}"),
    };
    assert_eq!(interaction.keyboard.pressed_keys, ["a"]);
    assert_eq!(interaction.keyboard.recent_keys, ["a"]);
    assert_eq!(interaction.mouse.buttons, ["left"]);
    assert_eq!((interaction.mouse.x, interaction.mouse.y), (-100, 200));
    assert_eq!(
        (interaction.mouse.norm_x, interaction.mouse.norm_y),
        (0.25, 0.75)
    );
    assert!((interaction.batch.motion.dx - 0.1).abs() < f32::EPSILON);
    assert!((interaction.batch.motion.dy + 0.05).abs() < f32::EPSILON);

    assert_eq!(published_events.len(), 3);
    assert_eq!(
        published_events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        [0, 0, 0]
    );
    assert!(published_events.iter().all(|event| event.at_ms == 1_000));
    assert!(published_events.iter().all(|event| event.repeat_count == 1));
    assert!(matches!(
        published_events[0].event,
        InputEvent::Key {
            ref source_id,
            ref key,
            state: InputButtonState::Pressed,
        } if source_id == "keyboard-1" && key == "a"
    ));
    assert_eq!(
        published_events[0].physical_code.as_deref(),
        Some("windows:set1:none:1e")
    );
    assert!(matches!(
        published_events[1].event,
        InputEvent::Key {
            state: InputButtonState::Repeated,
            ..
        }
    ));
    assert!(matches!(
        published_events[2].event,
        InputEvent::MouseButton {
            ref button,
            state: InputButtonState::Pressed,
            ..
        } if button == "left"
    ));

    let registry = statuses.snapshot();
    assert_eq!(registry.source_graph_generation(), graph.generation());
    assert_eq!(registry.handles().len(), 1);
    let status = registry.handles()[0].snapshot();
    assert_eq!(status.backend.as_ref(), "raw_input");
    assert_eq!(status.state, SourceState::Live);
    assert_eq!(status.freshness, SourceFreshness::NotApplicable);
    assert_eq!(status.source_graph_generation, graph.generation());
    assert_eq!(status.resource_count, 2);

    router.resolve_into(
        consumer,
        InteractionRouteRequest::host(),
        &[route_source],
        InteractionRouteContext {
            source_graph_generation: graph.generation(),
            now_ms: 1_001,
            ..InteractionRouteContext::default()
        },
        &mut routed,
    );
    assert_eq!(routed.interaction.keyboard.pressed_keys, ["a"]);
    assert_eq!(routed.interaction.mouse.buttons, ["left"]);
    assert_eq!(routed.diagnostics.selected.len(), 1);

    manager.stop_all();
    assert_eq!(registry.handles()[0].snapshot().state, SourceState::Stopped);
}

#[test]
fn capture_frame_flows_through_health_and_render_snapshot() {
    let config = CaptureConfig {
        target_fps: 1,
        grid_cols: 2,
        grid_rows: 1,
        smoothing_alpha: 1.0,
        ..CaptureConfig::default()
    };
    let (source, fixture) = ScreenCaptureFixtureSource::new(config);
    let mut manager = InputManager::new();
    let graph = manager.input_graph_handle();
    let statuses = manager.source_status_registry();
    manager.add_source(Box::new(source));
    manager.start_all().expect("fixture source starts");

    fixture.inject(
        vec![
            255, 0, 0, 255, 255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255, 255, 0, 0, 255, 255, 0,
            0, 255, 0, 0, 255, 255, 0, 0, 255, 255,
        ],
        4,
        2,
    );
    manager.sample_sources(1.0 / 30.0);

    let graph = graph.snapshot();
    assert_eq!(graph.slots().len(), 1);
    assert_eq!(graph.slots()[0].kind(), Some(SourceKind::Screen));
    let latest = graph.slots()[0].latest();
    let screen = match latest.as_deref() {
        Some(InputData::Screen(screen)) => screen,
        other => panic!("expected screen publication, got {other:?}"),
    };
    assert_eq!((screen.source_width, screen.source_height), (4, 2));
    assert_eq!((screen.grid_width, screen.grid_height), (2, 1));
    assert_eq!(screen.zone_colors.len(), 2);
    assert_eq!(screen.zone_colors[0].colors, [[255, 0, 0]]);
    assert_eq!(screen.zone_colors[1].colors, [[0, 0, 255]]);
    assert!(screen.canvas_downscale.is_some());

    let registry = statuses.snapshot();
    assert_eq!(registry.source_graph_generation(), graph.generation());
    assert_eq!(registry.handles().len(), 1);
    let status = registry.handles()[0].snapshot();
    assert_eq!(status.backend.as_ref(), "in_process");
    assert_eq!(status.state, SourceState::Live);
    assert_eq!(status.freshness, SourceFreshness::Fresh);
    assert_eq!(status.source_graph_generation, graph.generation());
    assert_eq!(status.resource_count, 1);

    manager.stop_all();
    assert_eq!(registry.handles()[0].snapshot().state, SourceState::Stopped);
}
