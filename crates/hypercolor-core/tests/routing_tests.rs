//! External contract tests for deterministic per-consumer interaction routing.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use hypercolor_core::input::routing::{
    ConsumerIncarnation, InteractionRouteCatalog, InteractionRouteContext, InteractionRouteRead,
    InteractionRouteRequest, InteractionRouteSlot, InteractionRouteSnapshot,
    InteractionRouteSource, InteractionRouteSourceClass, InteractionRouter, RoutedInteraction,
    SourceIncarnation,
};
use hypercolor_core::input::{
    BrowserConnectionIncarnation, BrowserInputChildKey, BrowserInputSource, BrowserPreviewId,
    InputData, InputEventRead, InputManager, InputSource, InteractionBatch, InteractionData,
    InteractionSource, InteractionSourceRole, InteractionTransientTotals, KeyboardData,
    ManagedSourceRole, MotionAggregate, MouseData, SourceRoleBinding,
};
use hypercolor_core::types::config::InteractionRoutePolicy;
use hypercolor_core::types::event::{InputButtonState, InputEvent, TimedInputEvent};

trait ResolveForTest {
    fn resolve(
        &mut self,
        consumer: ConsumerIncarnation,
        request: InteractionRouteRequest,
        sources: &[InteractionRouteSource],
        context: InteractionRouteContext,
    ) -> RoutedInteraction;
}

impl ResolveForTest for InteractionRouter {
    fn resolve(
        &mut self,
        consumer: ConsumerIncarnation,
        request: InteractionRouteRequest,
        sources: &[InteractionRouteSource],
        context: InteractionRouteContext,
    ) -> RoutedInteraction {
        let mut output = RoutedInteraction::new(consumer);
        self.resolve_into(consumer, request, sources, context, &mut output);
        output
    }
}

#[derive(Default)]
struct FakeSlot {
    state: Mutex<FakeSlotState>,
}

#[derive(Default)]
struct FakeSlotState {
    latest: Option<Arc<InteractionData>>,
    events: VecDeque<(u64, TimedInputEvent)>,
    next_cursor: u64,
    dropped_total: u64,
    capacity: usize,
    interaction_transients: InteractionTransientTotals,
}

impl FakeSlot {
    fn with_capacity(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(FakeSlotState {
                capacity,
                ..FakeSlotState::default()
            }),
        })
    }

    fn publish(&self, generation: u64, keys: &[&str], buttons: &[&str]) {
        let mut state = self.state.lock().expect("fake slot lock");
        let interaction = InteractionData {
            keyboard: KeyboardData {
                pressed_keys: keys.iter().map(ToString::to_string).collect(),
                ..KeyboardData::default()
            },
            mouse: MouseData {
                buttons: buttons.iter().map(ToString::to_string).collect(),
                down: !buttons.is_empty(),
                ..MouseData::default()
            },
            generation,
            ..InteractionData::default()
        };
        state.latest = Some(Arc::new(interaction));
    }

    fn push(&self, event: TimedInputEvent) {
        let mut state = self.state.lock().expect("fake slot lock");
        let cursor = state.next_cursor;
        state.next_cursor += 1;
        if state.events.len() == state.capacity {
            state.events.pop_front();
            state.dropped_total += 1;
        }
        state.events.push_back((cursor, event));
    }

    fn publish_motion(&self, generation: u64, dx: f32) {
        let mut state = self.state.lock().expect("fake slot lock");
        state.interaction_transients.dx += f64::from(dx);
        state.interaction_transients.distance += f64::from(dx.abs());
        state.latest = Some(Arc::new(InteractionData {
            batch: InteractionBatch {
                motion: MotionAggregate {
                    dx,
                    dy: 0.0,
                    distance: dx.abs(),
                },
                ..InteractionBatch::default()
            },
            generation,
            ..InteractionData::default()
        }));
    }

    fn clear_publication(&self) {
        self.state.lock().expect("fake slot lock").latest = None;
    }

    fn publish_wheel_snapshot(&self, generation: u64, delta_hi_res: i32) {
        self.state.lock().expect("fake slot lock").latest = Some(Arc::new(InteractionData {
            batch: InteractionBatch {
                wheel_hi_res: delta_hi_res,
                ..InteractionBatch::default()
            },
            generation,
            ..InteractionData::default()
        }));
    }

    fn report_losses(&self, dropped_events: u64, invalid_events: u64) {
        let mut state = self.state.lock().expect("fake slot lock");
        state.interaction_transients.dropped_events = state
            .interaction_transients
            .dropped_events
            .saturating_add(dropped_events);
        state.interaction_transients.invalid_events = state
            .interaction_transients
            .invalid_events
            .saturating_add(invalid_events);
    }
}

impl InteractionRouteSlot for FakeSlot {
    fn read_interaction_since(
        &self,
        cursor: u64,
        output: &mut Vec<TimedInputEvent>,
    ) -> InteractionRouteRead {
        let state = self.state.lock().expect("fake slot lock");
        let oldest = state
            .events
            .front()
            .map_or(state.next_cursor, |(cursor, _)| *cursor);
        let effective = cursor.min(state.next_cursor);
        let dropped = oldest.saturating_sub(effective);
        output.extend(
            state
                .events
                .iter()
                .filter(|(entry_cursor, _)| *entry_cursor >= effective.max(oldest))
                .map(|(_, event)| event.clone()),
        );
        InteractionRouteRead {
            snapshot: state
                .latest
                .clone()
                .map(InteractionRouteSnapshot::Interaction),
            events: InputEventRead {
                next_cursor: state.next_cursor,
                dropped,
                dropped_total: state.dropped_total,
            },
            interaction_transients: state.interaction_transients,
        }
    }
}

struct CatalogHostSource {
    running: bool,
}

impl InputSource for CatalogHostSource {
    fn name(&self) -> &'static str {
        "catalog-host"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

impl SourceRoleBinding for CatalogHostSource {
    type Role = InteractionSourceRole;
}

impl InteractionSource for CatalogHostSource {}

fn source(
    incarnation: SourceIncarnation,
    descriptor: &str,
    class: InteractionRouteSourceClass,
    revision: u64,
    slot: &Arc<FakeSlot>,
) -> InteractionRouteSource {
    InteractionRouteSource::new(
        incarnation,
        descriptor,
        class,
        revision,
        Arc::clone(slot) as Arc<dyn InteractionRouteSlot>,
    )
}

fn request(
    policy: InteractionRoutePolicy,
    browser_source: Option<SourceIncarnation>,
) -> InteractionRouteRequest {
    InteractionRouteRequest {
        policy,
        browser_source,
    }
}

fn context(now_ms: u64) -> InteractionRouteContext {
    InteractionRouteContext {
        config_generation: 11,
        source_graph_generation: 22,
        browser_registry_generation: 33,
        now_ms,
    }
}

#[test]
fn catalog_preserves_source_order_revisions_and_exact_browser_selection() {
    let manager = InputManager::new();
    manager
        .add_source(ManagedSourceRole::interaction(Box::new(
            CatalogHostSource { running: false },
        )))
        .expect("host source should register");
    let browser = BrowserInputSource::new();
    let browser_handle = browser.handle();
    let browser_registry = browser_handle.registry();
    manager
        .add_source(ManagedSourceRole::interaction(Box::new(browser)))
        .expect("browser source should register");
    manager.start_all().expect("catalog sources should start");
    let attachment = browser_handle
        .attach(BrowserInputChildKey::new(
            BrowserConnectionIncarnation::new(7),
            BrowserPreviewId::new("catalog-preview"),
        ))
        .expect("browser child should attach");
    let suppressed_attachment = browser_handle
        .attach(BrowserInputChildKey::new(
            BrowserConnectionIncarnation::new(8),
            BrowserPreviewId::new("catalog-suppressed"),
        ))
        .expect("second browser child should attach");
    let selected_descriptor = Arc::<str>::from(attachment.slot().source_id());
    let suppressed_descriptor = Arc::<str>::from(suppressed_attachment.slot().source_id());

    let graph = manager.input_graph_handle().snapshot();
    let browser = browser_registry.snapshot();
    let mut catalog = InteractionRouteCatalog::default();
    catalog.refresh(&graph, &browser, Instant::now());

    assert_eq!(
        catalog
            .sources()
            .iter()
            .map(|source| source.class)
            .collect::<Vec<_>>(),
        [
            InteractionRouteSourceClass::Host,
            InteractionRouteSourceClass::CompatibilityAggregate,
            InteractionRouteSourceClass::Browser,
            InteractionRouteSourceClass::Browser,
        ]
    );
    assert!(
        catalog
            .sources()
            .iter()
            .all(|source| source.availability_revision == 1)
    );

    let consumer = ConsumerIncarnation::new(91);
    let mut router = InteractionRouter::default();
    let mut output = RoutedInteraction::new(consumer);
    catalog.resolve_into(
        &mut router,
        consumer,
        InteractionRouteRequest {
            policy: InteractionRoutePolicy::Browser,
            browser_source: Some(SourceIncarnation::browser_child(
                attachment.publication_id().get(),
            )),
        },
        13,
        17,
        &mut output,
    );

    assert_eq!(output.diagnostics.selected.len(), 1);
    assert_eq!(
        output.diagnostics.selected[0].descriptor,
        selected_descriptor
    );
    assert_eq!(
        output.diagnostics.selected[0].incarnation,
        SourceIncarnation::browser_child(attachment.publication_id().get())
    );
    assert_eq!(
        output.diagnostics.source_graph_generation,
        graph.generation()
    );
    assert_eq!(
        output.diagnostics.browser_registry_generation,
        browser.generation()
    );
    assert!(
        output
            .diagnostics
            .suppressed
            .contains(&suppressed_descriptor)
    );
    assert_ne!(selected_descriptor, suppressed_descriptor);
    assert!(catalog.sources().iter().all(|source| {
        source.class != InteractionRouteSourceClass::Browser
            || source.descriptor.as_ref() != "browser_input"
    }));
    assert_eq!(
        output
            .diagnostics
            .suppressed
            .iter()
            .filter(|descriptor| descriptor.as_ref() == "browser_input")
            .count(),
        1,
        "only the diagnostic compatibility aggregate uses the shared id"
    );
    assert!(catalog.sources().iter().any(|source| {
        source.class == InteractionRouteSourceClass::CompatibilityAggregate
            && output
                .diagnostics
                .selected
                .iter()
                .all(|selected| selected.incarnation != source.incarnation)
    }));

    catalog.refresh(&graph, &browser, Instant::now());
    assert!(
        catalog
            .sources()
            .iter()
            .all(|source| source.availability_revision == 1)
    );
    manager
        .set_interaction_capture_active(false)
        .expect("interaction capture demand should update");
    catalog.refresh(&graph, &browser, Instant::now());
    assert_eq!(
        catalog
            .sources()
            .iter()
            .map(|source| source.availability_revision)
            .collect::<Vec<_>>(),
        [2, 1, 1, 1]
    );
}

fn key_event(source_id: &str, key: &str, state: InputButtonState, seq: u64) -> TimedInputEvent {
    TimedInputEvent {
        event: InputEvent::Key {
            source_id: source_id.to_owned(),
            key: key.to_owned(),
            state,
        },
        at_ms: seq,
        seq,
        physical_code: Some(format!("scan:{key}")),
        repeat_count: 1,
    }
}

fn wheel_event(source_id: &str, delta_hi_res: i32, seq: u64) -> TimedInputEvent {
    TimedInputEvent {
        event: InputEvent::MouseWheel {
            source_id: source_id.to_owned(),
            delta_hi_res,
        },
        at_ms: seq,
        seq,
        physical_code: Some("wheel".to_owned()),
        repeat_count: 1,
    }
}

fn event_keys(interaction: &InteractionData) -> Vec<(&str, InputButtonState)> {
    interaction
        .batch
        .events
        .iter()
        .filter_map(|event| match &event.event {
            InputEvent::Key { key, state, .. } => Some((key.as_str(), *state)),
            _ => None,
        })
        .collect()
}

#[test]
fn policies_select_exact_host_and_connection_scoped_browser_sources() {
    let host_id = SourceIncarnation::host_slot(7);
    let browser_a_id = SourceIncarnation::browser_child(7);
    let browser_b_id = SourceIncarnation::browser_child(8);
    let compatibility_id = SourceIncarnation::compatibility_aggregate(7);
    assert_ne!(host_id, browser_a_id, "source namespaces must never alias");

    let host = FakeSlot::with_capacity(16);
    let browser_a = FakeSlot::with_capacity(16);
    let browser_b = FakeSlot::with_capacity(16);
    let compatibility = FakeSlot::with_capacity(16);
    let sources = vec![
        source(
            host_id,
            "host:keyboard",
            InteractionRouteSourceClass::Host,
            1,
            &host,
        ),
        source(
            browser_a_id,
            "browser:a",
            InteractionRouteSourceClass::Browser,
            1,
            &browser_a,
        ),
        source(
            browser_b_id,
            "browser:b",
            InteractionRouteSourceClass::Browser,
            1,
            &browser_b,
        ),
        source(
            compatibility_id,
            "browser:compatibility-aggregate",
            InteractionRouteSourceClass::CompatibilityAggregate,
            1,
            &compatibility,
        ),
    ];
    let mut router = InteractionRouter::default();
    let host_consumer = ConsumerIncarnation::new(1);
    let preview_a = ConsumerIncarnation::new(2);
    let preview_b = ConsumerIncarnation::new(3);
    let merged = ConsumerIncarnation::new(4);

    let _ = router.resolve(
        host_consumer,
        request(InteractionRoutePolicy::Host, None),
        &sources,
        context(0),
    );
    let _ = router.resolve(
        preview_a,
        request(InteractionRoutePolicy::Browser, Some(browser_a_id)),
        &sources,
        context(0),
    );
    let _ = router.resolve(
        preview_b,
        request(InteractionRoutePolicy::Browser, Some(browser_b_id)),
        &sources,
        context(0),
    );
    let _ = router.resolve(
        merged,
        request(InteractionRoutePolicy::Merge, Some(browser_a_id)),
        &sources,
        context(0),
    );

    host.push(key_event("host", "h", InputButtonState::Pressed, 1));
    host.publish(1, &["h"], &[]);
    browser_a.push(key_event("a", "a", InputButtonState::Pressed, 2));
    browser_a.publish(1, &["a"], &[]);
    browser_b.push(key_event("b", "b", InputButtonState::Pressed, 3));
    browser_b.publish(1, &["b"], &[]);
    compatibility.push(key_event(
        "compatibility",
        "leak",
        InputButtonState::Pressed,
        4,
    ));
    compatibility.publish(1, &["leak"], &[]);

    let host_result = router.resolve(
        host_consumer,
        request(InteractionRoutePolicy::Host, None),
        &sources,
        context(4),
    );
    let preview_a_result = router.resolve(
        preview_a,
        request(InteractionRoutePolicy::Browser, Some(browser_a_id)),
        &sources,
        context(4),
    );
    let preview_b_result = router.resolve(
        preview_b,
        request(InteractionRoutePolicy::Browser, Some(browser_b_id)),
        &sources,
        context(4),
    );
    let merged_result = router.resolve(
        merged,
        request(InteractionRoutePolicy::Merge, Some(browser_a_id)),
        &sources,
        context(4),
    );

    assert_eq!(
        event_keys(&host_result.interaction),
        vec![("h", InputButtonState::Pressed)]
    );
    assert_eq!(
        event_keys(&preview_a_result.interaction),
        vec![("a", InputButtonState::Pressed)]
    );
    assert_eq!(
        event_keys(&preview_b_result.interaction),
        vec![("b", InputButtonState::Pressed)]
    );
    assert_eq!(
        event_keys(&merged_result.interaction),
        vec![
            ("h", InputButtonState::Pressed),
            ("a", InputButtonState::Pressed)
        ]
    );
    assert_eq!(merged_result.diagnostics.selected.len(), 2);
    assert_eq!(
        merged_result.diagnostics.suppressed.as_ref(),
        [
            Arc::<str>::from("browser:b"),
            Arc::<str>::from("browser:compatibility-aggregate")
        ]
    );
    for result in [
        &host_result,
        &preview_a_result,
        &preview_b_result,
        &merged_result,
    ] {
        assert!(
            !event_keys(&result.interaction)
                .iter()
                .any(|(key, _)| *key == "leak"),
            "compatibility aggregate must never be routable"
        );
    }
}

#[test]
fn cursors_start_at_tail_survive_rebuild_and_stay_independent() {
    let host_id = SourceIncarnation::host_slot(1);
    let replacement_id = SourceIncarnation::host_slot(2);
    let slot = FakeSlot::with_capacity(16);
    slot.push(key_event("host", "old", InputButtonState::Pressed, 1));
    slot.publish(1, &["old"], &[]);
    let mut sources = vec![source(
        host_id,
        "host:first",
        InteractionRouteSourceClass::Host,
        1,
        &slot,
    )];
    let mut router = InteractionRouter::default();
    let fast = ConsumerIncarnation::new(1);
    let slow = ConsumerIncarnation::new(2);

    let attached = router.resolve(fast, InteractionRouteRequest::host(), &sources, context(1));
    assert!(
        attached.interaction.batch.events.is_empty(),
        "attach must start at tail"
    );
    let _ = router.resolve(slow, InteractionRouteRequest::host(), &sources, context(1));

    slot.push(key_event("host", "old", InputButtonState::Released, 2));
    slot.publish(2, &[], &[]);
    slot.push(key_event("host", "new", InputButtonState::Pressed, 3));
    slot.publish(3, &["new"], &[]);
    let fast_first = router.resolve(fast, InteractionRouteRequest::host(), &sources, context(3));
    assert_eq!(
        event_keys(&fast_first.interaction),
        vec![("new", InputButtonState::Pressed)]
    );

    sources[0] = source(
        host_id,
        "host:renamed",
        InteractionRouteSourceClass::Host,
        2,
        &slot,
    );
    slot.push(key_event("host", "later", InputButtonState::Pressed, 4));
    slot.publish(4, &["new", "later"], &[]);
    let fast_second = router.resolve(fast, InteractionRouteRequest::host(), &sources, context(4));
    assert_eq!(
        event_keys(&fast_second.interaction),
        vec![("later", InputButtonState::Pressed)]
    );
    let slow_result = router.resolve(slow, InteractionRouteRequest::host(), &sources, context(4));
    assert_eq!(
        event_keys(&slow_result.interaction),
        vec![
            ("new", InputButtonState::Pressed),
            ("later", InputButtonState::Pressed)
        ]
    );

    let replacement = FakeSlot::with_capacity(16);
    replacement.push(key_event(
        "replacement",
        "stale",
        InputButtonState::Pressed,
        5,
    ));
    replacement.publish(1, &["stale"], &[]);
    sources[0] = source(
        replacement_id,
        "host:replacement",
        InteractionRouteSourceClass::Host,
        1,
        &replacement,
    );
    let replaced = router.resolve(fast, InteractionRouteRequest::host(), &sources, context(5));
    assert_eq!(
        event_keys(&replaced.interaction),
        vec![
            ("later", InputButtonState::Released),
            ("new", InputButtonState::Released)
        ]
    );
    assert!(
        !replaced
            .interaction
            .keyboard
            .pressed_keys
            .contains(&"stale".to_owned())
    );
}

#[test]
fn route_transitions_and_shared_controls_preserve_press_provenance() {
    let host_id = SourceIncarnation::host_slot(1);
    let browser_id = SourceIncarnation::browser_child(1);
    let host = FakeSlot::with_capacity(16);
    let browser = FakeSlot::with_capacity(16);
    let sources = vec![
        source(host_id, "host", InteractionRouteSourceClass::Host, 1, &host),
        source(
            browser_id,
            "browser",
            InteractionRouteSourceClass::Browser,
            1,
            &browser,
        ),
    ];
    let consumer = ConsumerIncarnation::new(1);
    let mut router = InteractionRouter::default();
    let _ = router.resolve(
        consumer,
        request(InteractionRoutePolicy::Merge, Some(browser_id)),
        &sources,
        context(0),
    );

    host.push(key_event("host", "x", InputButtonState::Pressed, 1));
    host.publish(1, &["x"], &[]);
    browser.push(key_event("browser", "x", InputButtonState::Pressed, 2));
    browser.publish(1, &["x"], &[]);
    let pressed = router.resolve(
        consumer,
        request(InteractionRoutePolicy::Merge, Some(browser_id)),
        &sources,
        context(2),
    );
    assert_eq!(
        event_keys(&pressed.interaction).len(),
        2,
        "explicit merge keeps both presses"
    );
    assert_eq!(pressed.interaction.keyboard.pressed_keys, vec!["x"]);

    let host_only = router.resolve(
        consumer,
        request(InteractionRoutePolicy::Host, None),
        &sources,
        context(3),
    );
    assert!(host_only.interaction.batch.events.is_empty());
    assert_eq!(host_only.interaction.keyboard.pressed_keys, vec!["x"]);

    let browser_only = router.resolve(
        consumer,
        request(InteractionRoutePolicy::Browser, Some(browser_id)),
        &sources,
        context(4),
    );
    assert_eq!(
        event_keys(&browser_only.interaction),
        vec![("x", InputButtonState::Released)]
    );
    assert!(browser_only.interaction.keyboard.pressed_keys.is_empty());
    assert!(
        browser_only.diagnostics.metrics.invalid_events == 0,
        "route synthesis is not an invalid event"
    );
}

#[test]
fn initial_holds_and_unobserved_releases_are_quarantined() {
    let browser_id = SourceIncarnation::browser_child(9);
    let browser = FakeSlot::with_capacity(16);
    browser.publish(1, &["held"], &[]);
    let sources = vec![source(
        browser_id,
        "browser",
        InteractionRouteSourceClass::Browser,
        1,
        &browser,
    )];
    let consumer = ConsumerIncarnation::new(1);
    let route = request(InteractionRoutePolicy::Browser, Some(browser_id));
    let mut router = InteractionRouter::default();
    let initial = router.resolve(consumer, route, &sources, context(0));
    assert!(initial.interaction.keyboard.pressed_keys.is_empty());

    browser.push(key_event("browser", "held", InputButtonState::Released, 1));
    browser.publish(2, &[], &[]);
    let released = router.resolve(consumer, route, &sources, context(1));
    assert!(released.interaction.batch.events.is_empty());
    assert_eq!(released.diagnostics.metrics.invalid_events, 0);

    browser.push(key_event("browser", "ghost", InputButtonState::Released, 2));
    let ghost = router.resolve(consumer, route, &sources, context(2));
    assert!(ghost.interaction.batch.events.is_empty());
    assert_eq!(ghost.diagnostics.metrics.invalid_events, 1);

    browser.push(key_event("browser", "held", InputButtonState::Pressed, 3));
    browser.publish(3, &["held"], &[]);
    let fresh = router.resolve(consumer, route, &sources, context(3));
    assert_eq!(
        event_keys(&fresh.interaction),
        vec![("held", InputButtonState::Pressed)]
    );
    assert_eq!(fresh.interaction.keyboard.pressed_keys, vec!["held"]);
}

#[test]
fn disappearing_snapshot_releases_observed_controls() {
    let host_id = SourceIncarnation::host_slot(1);
    let host = FakeSlot::with_capacity(4);
    let sources = vec![source(
        host_id,
        "host",
        InteractionRouteSourceClass::Host,
        1,
        &host,
    )];
    let consumer = ConsumerIncarnation::new(1);
    let mut router = InteractionRouter::default();
    let _ = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(0),
    );

    host.push(key_event("host", "a", InputButtonState::Pressed, 1));
    host.publish(1, &["a"], &[]);
    let pressed = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(1),
    );
    assert_eq!(
        event_keys(&pressed.interaction),
        vec![("a", InputButtonState::Pressed)]
    );

    host.clear_publication();
    let disappeared = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(2),
    );
    assert_eq!(
        event_keys(&disappeared.interaction),
        vec![("a", InputButtonState::Released)]
    );
    assert!(disappeared.interaction.keyboard.pressed_keys.is_empty());
    assert_eq!(disappeared.diagnostics.metrics.invalid_events, 0);
}

#[test]
fn overflow_synthesizes_release_quarantines_current_holds_and_tracks_separate_metrics() {
    let host_id = SourceIncarnation::host_slot(1);
    let host = FakeSlot::with_capacity(2);
    let sources = vec![source(
        host_id,
        "host",
        InteractionRouteSourceClass::Host,
        1,
        &host,
    )];
    let consumer = ConsumerIncarnation::new(1);
    let mut router = InteractionRouter::default();
    let _ = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(0),
    );

    host.push(key_event("host", "a", InputButtonState::Pressed, 1));
    host.publish(1, &["a"], &[]);
    let pressed = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(1),
    );
    assert_eq!(
        event_keys(&pressed.interaction),
        vec![("a", InputButtonState::Pressed)]
    );

    host.push(key_event("host", "b", InputButtonState::Pressed, 2));
    host.push(key_event("host", "b", InputButtonState::Released, 3));
    host.push(key_event("host", "c", InputButtonState::Pressed, 4));
    host.publish(4, &["a", "c"], &[]);
    let overflow = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(4),
    );
    assert_eq!(
        event_keys(&overflow.interaction),
        vec![("a", InputButtonState::Released)]
    );
    assert!(overflow.interaction.keyboard.pressed_keys.is_empty());
    assert_eq!(overflow.interaction.batch.dropped_events, 1);
    assert_eq!(overflow.diagnostics.metrics.overwritten_events, 1);
    assert_eq!(overflow.diagnostics.metrics.invalid_events, 0);

    host.push(key_event("host", "a", InputButtonState::Released, 5));
    host.publish(5, &["c"], &[]);
    let quarantined_release = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(5),
    );
    assert!(quarantined_release.interaction.batch.events.is_empty());

    host.push(key_event("host", "a", InputButtonState::Pressed, 6));
    host.publish(6, &["a", "c"], &[]);
    let fresh = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(6),
    );
    assert_eq!(
        event_keys(&fresh.interaction),
        vec![("a", InputButtonState::Pressed)]
    );
}

#[test]
fn route_generations_are_per_consumer_and_diagnostics_are_coherent() {
    let host_id = SourceIncarnation::host_slot(1);
    let browser_id = SourceIncarnation::browser_child(1);
    let host = FakeSlot::with_capacity(4);
    let browser = FakeSlot::with_capacity(4);
    let mut sources = vec![
        source(host_id, "host", InteractionRouteSourceClass::Host, 4, &host),
        source(
            browser_id,
            "browser",
            InteractionRouteSourceClass::Browser,
            5,
            &browser,
        ),
    ];
    let daemon = ConsumerIncarnation::new(1);
    let preview = ConsumerIncarnation::new(2);
    let mut router = InteractionRouter::default();
    let daemon_first = router.resolve(
        daemon,
        InteractionRouteRequest::host(),
        &sources,
        context(1),
    );
    let preview_first = router.resolve(
        preview,
        request(InteractionRoutePolicy::Browser, Some(browser_id)),
        &sources,
        context(1),
    );
    assert_eq!(daemon_first.diagnostics.route_generation, 1);
    assert_eq!(preview_first.diagnostics.route_generation, 1);
    let preview_stable = router.resolve(
        preview,
        request(InteractionRoutePolicy::Browser, Some(browser_id)),
        &sources,
        context(1),
    );
    assert!(Arc::ptr_eq(
        &preview_first.diagnostics,
        &preview_stable.diagnostics
    ));

    let daemon_changed = router.resolve(
        daemon,
        request(InteractionRoutePolicy::Merge, Some(browser_id)),
        &sources,
        context(2),
    );
    let mut later_context = context(2);
    later_context.config_generation = 12;
    later_context.source_graph_generation = 23;
    let preview_unchanged = router.resolve(
        preview,
        request(InteractionRoutePolicy::Browser, Some(browser_id)),
        &sources,
        later_context,
    );
    assert_eq!(daemon_changed.diagnostics.route_generation, 2);
    assert_eq!(preview_unchanged.diagnostics.route_generation, 1);
    assert_eq!(preview_unchanged.diagnostics.config_generation, 12);
    assert_eq!(preview_unchanged.diagnostics.source_graph_generation, 23);
    assert_eq!(
        preview_unchanged.diagnostics.browser_registry_generation,
        33
    );

    let prior_reuse = preview_unchanged.reuse_key;
    sources[1] = source(
        browser_id,
        "browser",
        InteractionRouteSourceClass::Browser,
        6,
        &browser,
    );
    let availability_changed = router.resolve(
        preview,
        request(InteractionRoutePolicy::Browser, Some(browser_id)),
        &sources,
        later_context,
    );
    assert_eq!(availability_changed.diagnostics.route_generation, 1);
    assert_ne!(availability_changed.reuse_key, prior_reuse);
}

#[test]
fn transient_motion_is_delivered_once_per_source_publication() {
    let host_id = SourceIncarnation::host_slot(1);
    let host = FakeSlot::with_capacity(4);
    let sources = vec![source(
        host_id,
        "host",
        InteractionRouteSourceClass::Host,
        1,
        &host,
    )];
    let consumer = ConsumerIncarnation::new(1);
    let mut router = InteractionRouter::default();
    let _ = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(0),
    );

    host.publish_motion(1, 0.25);
    let first = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(1),
    );
    let replay = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(2),
    );
    host.publish_motion(2, 0.5);
    let second = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(3),
    );

    assert_eq!(first.interaction.batch.motion.dx, 0.25);
    assert_eq!(replay.interaction.batch.motion, MotionAggregate::default());
    assert_eq!(replay.interaction.generation, first.interaction.generation);
    assert_eq!(second.interaction.batch.motion.dx, 0.5);
    assert!(second.interaction.generation > replay.interaction.generation);
}

#[test]
fn cumulative_motion_survives_different_consumer_cadences() {
    let host_id = SourceIncarnation::host_slot(1);
    let host = FakeSlot::with_capacity(4);
    let sources = vec![source(
        host_id,
        "host",
        InteractionRouteSourceClass::Host,
        1,
        &host,
    )];
    let fast = ConsumerIncarnation::new(1);
    let slow = ConsumerIncarnation::new(2);
    let mut router = InteractionRouter::default();
    let _ = router.resolve(fast, InteractionRouteRequest::host(), &sources, context(0));
    let _ = router.resolve(slow, InteractionRouteRequest::host(), &sources, context(0));

    host.publish_motion(1, 0.1);
    let fast_first = router.resolve(fast, InteractionRouteRequest::host(), &sources, context(1));
    host.publish_motion(2, 0.2);
    host.publish_motion(3, 0.3);
    let fast_second = router.resolve(fast, InteractionRouteRequest::host(), &sources, context(3));
    let slow_result = router.resolve(slow, InteractionRouteRequest::host(), &sources, context(3));

    assert!((fast_first.interaction.batch.motion.dx - 0.1).abs() < f32::EPSILON);
    assert!((fast_second.interaction.batch.motion.dx - 0.5).abs() < f32::EPSILON);
    assert!((slow_result.interaction.batch.motion.dx - 0.6).abs() < f32::EPSILON);
}

#[test]
fn transient_baselines_reset_on_detach_and_source_replacement() {
    let first_id = SourceIncarnation::host_slot(1);
    let second_id = SourceIncarnation::host_slot(2);
    let first = FakeSlot::with_capacity(4);
    let mut sources = vec![source(
        first_id,
        "host:first",
        InteractionRouteSourceClass::Host,
        1,
        &first,
    )];
    let consumer = ConsumerIncarnation::new(1);
    let mut router = InteractionRouter::default();
    let _ = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(0),
    );
    first.publish_motion(1, 0.1);
    let delivered = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(1),
    );
    assert!((delivered.interaction.batch.motion.dx - 0.1).abs() < f32::EPSILON);

    let _ = router.resolve(
        consumer,
        request(InteractionRoutePolicy::Browser, None),
        &sources,
        context(2),
    );
    first.publish_motion(2, 0.2);
    let reattached = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(3),
    );
    assert_eq!(
        reattached.interaction.batch.motion,
        MotionAggregate::default()
    );

    let replacement = FakeSlot::with_capacity(4);
    replacement.publish_motion(1, 0.4);
    sources[0] = source(
        second_id,
        "host:replacement",
        InteractionRouteSourceClass::Host,
        1,
        &replacement,
    );
    let replaced = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(4),
    );
    assert_eq!(
        replaced.interaction.batch.motion,
        MotionAggregate::default()
    );
    replacement.publish_motion(2, 0.5);
    let replacement_delta = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(5),
    );
    assert!((replacement_delta.interaction.batch.motion.dx - 0.5).abs() < f32::EPSILON);
}

#[test]
fn wheel_is_reconstructed_from_retained_events_without_snapshot_double_counting() {
    let host_id = SourceIncarnation::host_slot(1);
    let host = FakeSlot::with_capacity(4);
    let sources = vec![source(
        host_id,
        "host",
        InteractionRouteSourceClass::Host,
        1,
        &host,
    )];
    let consumer = ConsumerIncarnation::new(1);
    let mut router = InteractionRouter::default();
    let _ = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(0),
    );

    host.push(wheel_event("host", 120, 1));
    host.push(wheel_event("host", -30, 2));
    host.publish_wheel_snapshot(1, 90);
    let wheel = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(2),
    );
    let replay = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(3),
    );

    assert_eq!(wheel.interaction.batch.wheel_hi_res, 90);
    assert_eq!(replay.interaction.batch.wheel_hi_res, 0);
}

#[test]
fn reuse_key_advances_for_transients_when_source_generation_is_unchanged() {
    let host_id = SourceIncarnation::host_slot(1);
    let host = FakeSlot::with_capacity(4);
    host.publish(7, &[], &[]);
    let sources = vec![source(
        host_id,
        "host",
        InteractionRouteSourceClass::Host,
        1,
        &host,
    )];
    let consumer = ConsumerIncarnation::new(1);
    let mut router = InteractionRouter::default();
    let baseline = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(0),
    );

    host.publish_motion(7, 0.25);
    let motion = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(1),
    );
    host.push(wheel_event("host", 120, 2));
    host.publish_wheel_snapshot(7, 120);
    let wheel = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(2),
    );

    assert_eq!(baseline.reuse_key.sources, motion.reuse_key.sources);
    assert_eq!(motion.reuse_key.sources, wheel.reuse_key.sources);
    assert_eq!(
        baseline.reuse_key.route_generation,
        motion.reuse_key.route_generation
    );
    assert_eq!(
        motion.reuse_key.route_generation,
        wheel.reuse_key.route_generation
    );
    assert_ne!(baseline.reuse_key, motion.reuse_key);
    assert_ne!(motion.reuse_key, wheel.reuse_key);
    assert_eq!(motion.interaction.batch.motion.dx, 0.25);
    assert_eq!(wheel.interaction.batch.wheel_hi_res, 120);
}

#[test]
fn equal_timestamp_events_keep_selected_source_order() {
    let host_id = SourceIncarnation::host_slot(1);
    let browser_id = SourceIncarnation::browser_child(1);
    let host = FakeSlot::with_capacity(4);
    let browser = FakeSlot::with_capacity(4);
    let sources = vec![
        source(host_id, "host", InteractionRouteSourceClass::Host, 1, &host),
        source(
            browser_id,
            "browser",
            InteractionRouteSourceClass::Browser,
            1,
            &browser,
        ),
    ];
    let consumer = ConsumerIncarnation::new(1);
    let mut router = InteractionRouter::default();
    let route = request(InteractionRoutePolicy::Merge, Some(browser_id));
    let _ = router.resolve(consumer, route, &sources, context(0));

    host.push(wheel_event("host", 120, 5));
    browser.push(wheel_event("browser", -120, 5));
    let resolved = router.resolve(consumer, route, &sources, context(5));
    let source_ids = resolved
        .interaction
        .batch
        .events
        .iter()
        .map(|event| event.event.source_id())
        .collect::<Vec<_>>();

    assert_eq!(source_ids, ["host", "browser"]);
}

#[test]
fn producer_and_invalid_losses_remain_separate() {
    let host_id = SourceIncarnation::host_slot(1);
    let host = FakeSlot::with_capacity(4);
    let sources = vec![source(
        host_id,
        "host",
        InteractionRouteSourceClass::Host,
        1,
        &host,
    )];
    let consumer = ConsumerIncarnation::new(1);
    let mut router = InteractionRouter::default();
    let _ = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(0),
    );

    host.report_losses(3, 2);
    let losses = router.resolve(
        consumer,
        InteractionRouteRequest::host(),
        &sources,
        context(1),
    );

    assert_eq!(losses.interaction.batch.dropped_events, 5);
    assert_eq!(losses.diagnostics.metrics.overwritten_events, 0);
    assert_eq!(losses.diagnostics.metrics.producer_dropped_events, 3);
    assert_eq!(losses.diagnostics.metrics.invalid_events, 2);
}

#[test]
fn removing_consumer_synthesizes_one_release_per_logical_control() {
    let host_id = SourceIncarnation::host_slot(1);
    let browser_id = SourceIncarnation::browser_child(1);
    let host = FakeSlot::with_capacity(4);
    let browser = FakeSlot::with_capacity(4);
    let sources = vec![
        source(host_id, "host", InteractionRouteSourceClass::Host, 1, &host),
        source(
            browser_id,
            "browser",
            InteractionRouteSourceClass::Browser,
            1,
            &browser,
        ),
    ];
    let consumer = ConsumerIncarnation::new(1);
    let route = request(InteractionRoutePolicy::Merge, Some(browser_id));
    let mut router = InteractionRouter::default();
    let _ = router.resolve(consumer, route, &sources, context(0));
    host.push(key_event("host", "x", InputButtonState::Pressed, 1));
    host.publish(1, &["x"], &[]);
    browser.push(key_event("browser", "x", InputButtonState::Pressed, 2));
    browser.publish(1, &["x"], &[]);
    let _ = router.resolve(consumer, route, &sources, context(2));

    let released = router.remove_consumer(consumer, 10);

    assert_eq!(released.len(), 1);
    assert_eq!(released[0].at_ms, 10);
    assert_eq!(
        event_keys(&InteractionData {
            batch: hypercolor_core::input::InteractionBatch {
                events: released,
                ..Default::default()
            },
            ..Default::default()
        }),
        vec![("x", InputButtonState::Released)]
    );
}
