use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::anyhow;
use hypercolor_macos_capture::{
    MacosCaptureContentStyle, MacosCaptureFrame, MacosCaptureSelection, MacosDisplayClock,
    MacosFrameEvent, MacosFrameMailbox, MacosFrameStatus,
    MacosProtectedSourceState as NativeProtectedSourceState,
};

#[cfg(target_os = "macos")]
use hypercolor_macos_capture::{
    MacosCaptureCadence, MacosScreenCaptureSession, MacosStreamRequest,
};

use super::{
    CaptureConfig, CaptureCursor, CaptureCursorContent, CaptureDamage, CaptureFrame,
    CaptureFrameMetadata, CapturePixelFormat, CapturePlanePool, CaptureRotation, CaptureSourceId,
    CaptureStorage, CpuCaptureStorage, PixelExtent, PixelRect, RawCaptureSurface,
    ScreenAnalysisComputeCapacity, ScreenAnalysisResourcePlan, ScreenAnalysisWorkPlan,
    ScreenByteAdmissionCoordinator, ScreenCaptureDemand, ScreenCaptureInput, SourceScale,
    analyze_screen_frame,
};
use crate::input::status::SourceSessionSlot;
use crate::input::traits::{InputData, InputSource};
use crate::input::{
    MacosAuthorizationState, MacosCapabilityOwner, MacosProtectedSourceState,
    MacosScreenPlatformStatus, MacosSelectionState, SourceKind, SourcePlatformStatus,
    SourceStatusHandle, SourceStatusReporter,
};

const WORKER_WAIT: Duration = Duration::from_millis(100);

trait MacosCaptureControl: Send + Sync {
    fn mailbox(&self) -> MacosFrameMailbox;
    fn set_active(&self, active: bool);
    fn present_picker(&self) -> anyhow::Result<()>;
    fn request_authorization(&self) -> NativeProtectedSourceState;
    fn status(&self) -> NativeProtectedSourceState;
    fn selection(&self) -> MacosCaptureSelection;
    fn authorization(&self) -> MacosAuthorizationState;
    fn captured_at(&self, display_time: u64) -> anyhow::Result<Instant>;
}

#[cfg(target_os = "macos")]
struct NativeCaptureControl {
    session: MacosScreenCaptureSession,
    clock: MacosDisplayClock,
}

#[cfg(target_os = "macos")]
impl MacosCaptureControl for NativeCaptureControl {
    fn mailbox(&self) -> MacosFrameMailbox {
        self.session.mailbox()
    }

    fn set_active(&self, active: bool) {
        self.session.set_capture_active(active);
    }

    fn present_picker(&self) -> anyhow::Result<()> {
        self.session.present_picker().map_err(anyhow::Error::from)
    }

    fn request_authorization(&self) -> NativeProtectedSourceState {
        self.session.request_authorization()
    }

    fn status(&self) -> NativeProtectedSourceState {
        self.session.status()
    }

    fn selection(&self) -> MacosCaptureSelection {
        self.session.selection()
    }

    fn authorization(&self) -> MacosAuthorizationState {
        if MacosScreenCaptureSession::screen_authorized() {
            MacosAuthorizationState::Authorized
        } else if self.session.status() == NativeProtectedSourceState::PermissionDenied {
            MacosAuthorizationState::Denied
        } else {
            MacosAuthorizationState::NotDetermined
        }
    }

    fn captured_at(&self, display_time: u64) -> anyhow::Result<Instant> {
        self.clock
            .timestamp(display_time)
            .map_err(anyhow::Error::from)
    }
}

#[derive(Default)]
struct MacosPublication {
    worker_generation: u64,
    latest: Option<Arc<InputData>>,
}

struct PreparedWorker {
    analyzer: ScreenCaptureInput,
    plane_pool: CapturePlanePool,
    target_fps: u32,
}

struct CaptureWorker {
    stop: Arc<AtomicBool>,
    exit_rx: mpsc::Receiver<anyhow::Result<()>>,
    join: Option<thread::JoinHandle<()>>,
}

pub struct MacosScreenCaptureInput {
    config: CaptureConfig,
    control: Arc<dyn MacosCaptureControl>,
    admission: ScreenByteAdmissionCoordinator,
    publication: Arc<Mutex<MacosPublication>>,
    worker: Option<CaptureWorker>,
    worker_generation: u64,
    demand: ScreenCaptureDemand,
    running: bool,
    status: SourceStatusReporter,
    status_session: SourceSessionSlot,
    owner: MacosCapabilityOwner,
}

impl MacosScreenCaptureInput {
    #[cfg(target_os = "macos")]
    pub fn new(
        config: CaptureConfig,
        admission: ScreenByteAdmissionCoordinator,
    ) -> anyhow::Result<Self> {
        let request = MacosStreamRequest::new(
            MacosCaptureCadence::FramesPerSecond(config.target_fps),
            true,
        )?;
        let session = MacosScreenCaptureSession::new(request)?;
        let clock = MacosDisplayClock::system()?;
        Ok(Self::with_control(
            config,
            admission,
            Arc::new(NativeCaptureControl { session, clock }),
        ))
    }

    fn with_control(
        config: CaptureConfig,
        admission: ScreenByteAdmissionCoordinator,
        control: Arc<dyn MacosCaptureControl>,
    ) -> Self {
        let consented = control.authorization() == MacosAuthorizationState::Authorized;
        let mut source = Self {
            config,
            control,
            admission,
            publication: Arc::new(Mutex::new(MacosPublication::default())),
            worker: None,
            worker_generation: 0,
            demand: ScreenCaptureDemand::Inactive,
            running: false,
            status: SourceStatusReporter::new(
                "macos:session",
                SourceKind::Screen,
                "screen_capture_kit_cpu",
                true,
                consented,
                false,
            ),
            status_session: SourceSessionSlot::new(),
            owner: MacosCapabilityOwner::Standalone,
        };
        source
            .refresh_platform_status()
            .expect("new macOS screen status is not retired");
        source
    }

    pub fn authorize(&mut self) -> anyhow::Result<NativeProtectedSourceState> {
        let state = self.control.request_authorization();
        self.refresh_policy()?;
        self.refresh_platform_status()?;
        Ok(state)
    }

    pub fn present_picker(&mut self) -> anyhow::Result<()> {
        let result = self.control.present_picker();
        self.refresh_platform_status()?;
        result
    }

    pub fn protected_state(&self) -> NativeProtectedSourceState {
        self.control.status()
    }

    pub fn set_capability_owner(&mut self, owner: MacosCapabilityOwner) -> anyhow::Result<()> {
        self.owner = owner;
        self.refresh_platform_status()
    }

    fn refresh_platform_status(&mut self) -> anyhow::Result<()> {
        let state = self.control.status();
        self.status
            .set_platform(Some(SourcePlatformStatus::MacosScreen(
                MacosScreenPlatformStatus {
                    state: map_protected_state(state),
                    tcc: self.control.authorization(),
                    owner: self.owner,
                    selection: map_selection(self.control.selection()),
                    tahoe_selection: None,
                    owner_conflict: None,
                },
            )))?;
        Ok(())
    }

    fn refresh_policy(&mut self) -> anyhow::Result<()> {
        self.refresh_policy_for(self.demand)
    }

    fn refresh_policy_for(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        let consented = self.control.authorization() == MacosAuthorizationState::Authorized;
        self.status
            .set_policy(true, consented, demand.is_active())?;
        Ok(())
    }

    fn prepare_worker(&self, extent: PixelExtent) -> anyhow::Result<PreparedWorker> {
        let mut analyzer = ScreenCaptureInput::with_requested_extent_and_admission(
            self.config.clone(),
            extent,
            self.admission.clone(),
        )?;
        analyzer.start()?;
        Ok(PreparedWorker {
            analyzer,
            plane_pool: CapturePlanePool::with_admission_coordinator(self.admission.clone()),
            target_fps: self.config.target_fps,
        })
    }

    fn install_worker(&mut self, prepared: PreparedWorker) -> anyhow::Result<()> {
        let worker_generation = self
            .worker_generation
            .checked_add(1)
            .ok_or_else(|| anyhow!("macOS capture worker generation exhausted"))?;
        let mailbox = self.control.mailbox();
        let control = Arc::clone(&self.control);
        let publication = Arc::clone(&self.publication);
        let status_session = self.status_session.clone();
        let target_fps = prepared.target_fps;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let start = Arc::new(AtomicBool::new(false));
        let worker_start = Arc::clone(&start);
        let (exit_tx, exit_rx) = mpsc::channel();
        let join = thread::Builder::new()
            .name("hypercolor-macos-screen-capture".to_owned())
            .spawn(move || {
                while !worker_start.load(Ordering::Acquire) {
                    thread::park();
                }
                let result = if worker_stop.load(Ordering::Acquire) {
                    Ok(())
                } else {
                    run_worker(
                        prepared,
                        mailbox,
                        publication,
                        worker_generation,
                        target_fps,
                        status_session,
                        worker_stop,
                        control,
                    )
                };
                let _ = exit_tx.send(result);
            })?;
        self.stop_worker();
        self.worker_generation = worker_generation;
        {
            let mut publication = lock(&self.publication);
            publication.worker_generation = worker_generation;
            publication.latest = None;
        }
        self.worker = Some(CaptureWorker {
            stop,
            exit_rx,
            join: Some(join),
        });
        start.store(true, Ordering::Release);
        self.worker
            .as_ref()
            .and_then(|worker| worker.join.as_ref())
            .expect("installed worker retains its thread handle")
            .thread()
            .unpark();
        Ok(())
    }

    fn stop_worker(&mut self) {
        let Some(mut worker) = self.worker.take() else {
            return;
        };
        worker.stop.store(true, Ordering::Release);
        if let Some(join) = worker.join.take() {
            let _ = join.join();
        }
        lock(&self.publication).latest = None;
    }

    fn observe_worker_exit(&mut self) -> anyhow::Result<()> {
        let Some(worker) = self.worker.as_ref() else {
            return Ok(());
        };
        match worker.exit_rx.try_recv() {
            Ok(Ok(())) => {
                self.stop_worker();
                if self.running && self.demand.is_active() {
                    return Err(anyhow!("macOS capture worker exited while active"));
                }
            }
            Ok(Err(error)) => {
                self.stop_worker();
                return Err(error);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.stop_worker();
                return Err(anyhow!("macOS capture worker disconnected"));
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        Ok(())
    }
}

impl InputSource for MacosScreenCaptureInput {
    fn name(&self) -> &'static str {
        "macos_screen_capture"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.running {
            return Ok(());
        }
        self.refresh_policy()?;
        if let Some(extent) = self.demand.requested_extent() {
            let prepared = self.prepare_worker(extent)?;
            let session = self.status.begin_session()?;
            if let Err(error) = self.install_worker(prepared) {
                self.status.stop();
                return Err(error);
            }
            if let Some(session) = session {
                self.status_session.store(session);
            }
            self.control.set_active(true);
        }
        self.refresh_platform_status()?;
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.control.set_active(false);
        self.refresh_platform_status()
            .expect("live macOS screen status is not retired");
        self.status_session.clear();
        self.stop_worker();
        self.status.stop();
        self.demand = ScreenCaptureDemand::Inactive;
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        self.refresh_platform_status()?;
        self.observe_worker_exit()?;
        if !self.running || !self.demand.is_active() {
            return Ok(InputData::None);
        }
        let publication = lock(&self.publication);
        if publication.worker_generation != self.worker_generation {
            return Ok(InputData::None);
        }
        Ok(publication
            .latest
            .as_deref()
            .cloned()
            .unwrap_or(InputData::None))
    }

    fn sample_shared_and_drain_into(
        &mut self,
        _delta_secs: f32,
        _events: &mut Vec<crate::types::event::TimedInputEvent>,
    ) -> anyhow::Result<Option<Arc<InputData>>> {
        self.refresh_platform_status()?;
        self.observe_worker_exit()?;
        if !self.running || !self.demand.is_active() {
            return Ok(None);
        }
        let publication = lock(&self.publication);
        Ok((publication.worker_generation == self.worker_generation)
            .then(|| publication.latest.clone())
            .flatten())
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }

    fn is_screen_source(&self) -> bool {
        true
    }

    fn screen_capture_demand(&self) -> ScreenCaptureDemand {
        self.demand
    }

    fn screen_analysis_resource_plan(
        &self,
        demand: ScreenCaptureDemand,
    ) -> anyhow::Result<Option<ScreenAnalysisResourcePlan>> {
        let Some(extent) = demand.requested_extent() else {
            return Ok(None);
        };
        Ok(Some(ScreenAnalysisResourcePlan::try_new_for_extent(
            self.config.grid_cols,
            self.config.grid_rows,
            self.config.target_fps,
            extent,
            u64::MAX,
        )?))
    }

    fn screen_analysis_work_plan(
        &self,
        demand: ScreenCaptureDemand,
    ) -> anyhow::Result<Option<ScreenAnalysisWorkPlan>> {
        let Some(extent) = demand.requested_extent() else {
            return Ok(None);
        };
        Ok(Some(ScreenAnalysisWorkPlan::try_new(
            extent,
            extent,
            &self.config,
        )?))
    }

    fn screen_analysis_compute_capacity(&self) -> Option<ScreenAnalysisComputeCapacity> {
        None
    }

    fn set_screen_capture_demand(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        let prepared = demand
            .requested_extent()
            .map(|extent| self.prepare_worker(extent))
            .transpose()?;
        let was_active = self.demand.is_active();
        if !self.running {
            self.refresh_policy_for(demand)?;
            self.demand = demand;
            return Ok(());
        }
        if let Some(prepared) = prepared {
            let session = if was_active {
                None
            } else {
                self.refresh_policy_for(demand)?;
                self.status.begin_session()?
            };
            if let Err(error) = self.install_worker(prepared) {
                if !was_active {
                    self.refresh_policy_for(self.demand)?;
                }
                return Err(error);
            }
            if let Some(session) = session {
                self.status_session.store(session);
            }
            self.control.set_active(true);
        } else {
            self.control.set_active(false);
            self.status_session.clear();
            self.stop_worker();
            self.refresh_policy_for(demand)?;
        }
        self.demand = demand;
        self.refresh_platform_status()?;
        Ok(())
    }

    fn reconfigure_screen_capture(&mut self, config: &CaptureConfig) -> anyhow::Result<()> {
        let prepared = self
            .demand
            .requested_extent()
            .map(|extent| {
                let mut analyzer = ScreenCaptureInput::with_requested_extent_and_admission(
                    config.clone(),
                    extent,
                    self.admission.clone(),
                )?;
                analyzer.start()?;
                Ok::<_, anyhow::Error>(PreparedWorker {
                    analyzer,
                    plane_pool: CapturePlanePool::with_admission_coordinator(
                        self.admission.clone(),
                    ),
                    target_fps: config.target_fps,
                })
            })
            .transpose()?;
        if self.running
            && let Some(prepared) = prepared
        {
            self.install_worker(prepared)?;
        }
        self.config.clone_from(config);
        Ok(())
    }

    fn reselect_screen_source(&mut self) -> anyhow::Result<()> {
        self.present_picker()
    }
}

impl Drop for MacosScreenCaptureInput {
    fn drop(&mut self) {
        self.control.set_active(false);
        self.stop_worker();
    }
}

fn run_worker(
    mut prepared: PreparedWorker,
    mailbox: MacosFrameMailbox,
    publication: Arc<Mutex<MacosPublication>>,
    worker_generation: u64,
    target_fps: u32,
    status_session: SourceSessionSlot,
    stop: Arc<AtomicBool>,
    control: Arc<dyn MacosCaptureControl>,
) -> anyhow::Result<()> {
    let source_id = CaptureSourceId::new(Arc::<str>::from("macos:session"))?;
    let mut topology = TopologyState::default();
    while !stop.load(Ordering::Acquire) {
        let Some(delivery) = mailbox.wait_latest(WORKER_WAIT) else {
            continue;
        };
        match delivery {
            Ok(MacosFrameEvent::Frame(frame)) => {
                publish_frame(
                    &mut prepared,
                    *frame,
                    &source_id,
                    &mut topology,
                    &publication,
                    worker_generation,
                    target_fps,
                    &status_session,
                    &control,
                )?;
            }
            Ok(MacosFrameEvent::Lifecycle(
                MacosFrameStatus::Suspended | MacosFrameStatus::Stopped,
            ))
            | Err(_) => lock(&publication).latest = None,
            Ok(MacosFrameEvent::Lifecycle(_)) => {}
        }
    }
    prepared.analyzer.stop();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn publish_frame(
    prepared: &mut PreparedWorker,
    frame: MacosCaptureFrame,
    source_id: &CaptureSourceId,
    topology: &mut TopologyState,
    publication: &Mutex<MacosPublication>,
    worker_generation: u64,
    target_fps: u32,
    status_session: &SourceSessionSlot,
    control: &Arc<dyn MacosCaptureControl>,
) -> anyhow::Result<()> {
    let extent = PixelExtent::new(frame.storage_extent.width, frame.storage_extent.height)?;
    let row_stride = usize::try_from(extent.width())
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| anyhow!("macOS capture row stride overflow"))?;
    let byte_len = row_stride
        .checked_mul(usize::try_from(extent.height())?)
        .ok_or_else(|| anyhow!("macOS capture plane length overflow"))?;
    let mut plane = prepared.plane_pool.try_acquire(byte_len)?;
    plane.resize(byte_len, 0);
    frame.convert_bgra8_sdr_to_rgba8(&mut plane, row_stride)?;
    let captured_at = control.captured_at(frame.display_time)?;
    let fresh_until = captured_at
        .checked_add(Duration::from_nanos(
            2_000_000_000_u64.div_ceil(u64::from(target_fps)),
        ))
        .ok_or_else(|| anyhow!("macOS capture freshness deadline overflow"))?;
    let topology_generation = topology.observe(&frame)?;
    let geometry = super::CaptureGeometry::new(
        capture_origin(&frame)?,
        extent,
        extent,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    )?;
    let cursor = CaptureCursor {
        visible: frame.cursor_composed,
        position: None,
        hotspot: None,
        shape_extent: None,
        shape_generation: None,
        content: if frame.cursor_composed {
            CaptureCursorContent::Composed
        } else {
            CaptureCursorContent::Hidden
        },
    };
    let damage = CaptureDamage::new(
        frame
            .damage
            .iter()
            .map(|rect| {
                Ok(PixelRect::new(
                    u32::try_from(rect.x)?,
                    u32::try_from(rect.y)?,
                    rect.width,
                    rect.height,
                )?)
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
        Vec::new(),
    );
    let sequence = frame
        .sequence
        .checked_add(1)
        .ok_or_else(|| anyhow!("macOS capture sequence exhausted"))?;
    let capture = CaptureFrame::<RawCaptureSurface>::new(
        CaptureFrameMetadata {
            source_id: source_id.clone(),
            topology_generation,
            session_generation: frame.epoch,
            sequence,
            captured_at,
            fresh_until,
            geometry,
            colorimetry: super::CaptureColorimetry::SRGB,
            cursor,
        },
        CaptureStorage::Cpu(CpuCaptureStorage::from_owner(
            plane.freeze(),
            CapturePixelFormat::Rgba8,
            i64::try_from(row_stride)?,
            0,
        )),
        damage,
    )?;
    let snapshot = analyze_screen_frame(&mut prepared.analyzer, capture)?;
    if snapshot.geometry_frame().metadata().topology_generation != topology_generation {
        return Err(anyhow!("macOS analysis changed topology generation"));
    }
    let data = Arc::new(InputData::Screen(snapshot.data().clone()));
    if lock(publication).worker_generation != worker_generation {
        return Ok(());
    }
    if let Some(status) = status_session.load() {
        status.record_sample(captured_at, fresh_until, 1)?;
    }
    {
        let mut publication = lock(publication);
        if publication.worker_generation != worker_generation {
            return Ok(());
        }
        publication.latest = Some(data);
    }
    Ok(())
}

#[derive(Default)]
struct TopologyState {
    descriptor: Option<TopologyDescriptor>,
    generation: u64,
}

impl TopologyState {
    fn observe(&mut self, frame: &MacosCaptureFrame) -> anyhow::Result<u64> {
        let descriptor = TopologyDescriptor::from_frame(frame);
        if self.descriptor.as_ref() != Some(&descriptor) {
            self.generation = self
                .generation
                .checked_add(1)
                .ok_or_else(|| anyhow!("macOS topology generation exhausted"))?;
            self.descriptor = Some(descriptor);
        }
        Ok(self.generation)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TopologyDescriptor {
    width: u32,
    height: u32,
    content: (i64, i64, u32, u32),
    scale_bits: u64,
    screen: Option<(u64, u64, u64, u64)>,
}

impl TopologyDescriptor {
    fn from_frame(frame: &MacosCaptureFrame) -> Self {
        let content = frame.geometry.content_rect_pixels;
        Self {
            width: frame.storage_extent.width,
            height: frame.storage_extent.height,
            content: (content.x, content.y, content.width, content.height),
            scale_bits: frame.geometry.display_scale_factor.get().to_bits(),
            screen: frame.geometry.screen_rect_points.map(|rect| {
                (
                    rect.x.to_bits(),
                    rect.y.to_bits(),
                    rect.width.to_bits(),
                    rect.height.to_bits(),
                )
            }),
        }
    }
}

fn capture_origin(frame: &MacosCaptureFrame) -> anyhow::Result<super::PhysicalOrigin> {
    let rect = frame
        .geometry
        .screen_rect_points
        .unwrap_or(frame.geometry.content_rect_points);
    let scale = frame.geometry.display_scale_factor.get();
    Ok(super::PhysicalOrigin {
        x: scaled_coordinate(rect.x, scale)?,
        y: scaled_coordinate(rect.y, scale)?,
    })
}

fn scaled_coordinate(value: f64, scale: f64) -> anyhow::Result<i32> {
    let value = (value * scale).floor();
    if !value.is_finite() || value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
        return Err(anyhow!("macOS capture origin exceeds i32"));
    }
    Ok(value as i32)
}

const fn map_protected_state(state: NativeProtectedSourceState) -> MacosProtectedSourceState {
    match state {
        NativeProtectedSourceState::Disabled => MacosProtectedSourceState::Disabled,
        NativeProtectedSourceState::NeedsUserAction => MacosProtectedSourceState::NeedsUserAction,
        NativeProtectedSourceState::PermissionDenied => MacosProtectedSourceState::PermissionDenied,
        NativeProtectedSourceState::NeedsProcessRestart => {
            MacosProtectedSourceState::NeedsProcessRestart
        }
        NativeProtectedSourceState::NeedsSelection => MacosProtectedSourceState::NeedsSelection,
        NativeProtectedSourceState::ReadyIdle => MacosProtectedSourceState::ReadyIdle,
        NativeProtectedSourceState::Starting => MacosProtectedSourceState::Starting,
        NativeProtectedSourceState::Live => MacosProtectedSourceState::Live,
        NativeProtectedSourceState::Interrupted => MacosProtectedSourceState::Interrupted,
        NativeProtectedSourceState::Revoked => MacosProtectedSourceState::Revoked,
        NativeProtectedSourceState::Failed => MacosProtectedSourceState::Failed,
    }
}

fn map_selection(selection: MacosCaptureSelection) -> MacosSelectionState {
    match selection {
        MacosCaptureSelection::None => MacosSelectionState::None,
        MacosCaptureSelection::Display { source_id } => MacosSelectionState::Display { source_id },
        MacosCaptureSelection::SessionScoped { content_style } => {
            let content_style = match content_style {
                MacosCaptureContentStyle::Window => "window",
                MacosCaptureContentStyle::MultipleWindows => "multiple_windows",
                MacosCaptureContentStyle::Application => "application",
                MacosCaptureContentStyle::MultipleApplications => "multiple_applications",
                MacosCaptureContentStyle::Mixed => "mixed",
            };
            MacosSelectionState::SessionScoped {
                content_style: Arc::from(content_style),
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(feature = "macos-capture-fixtures")]
struct FixtureControl {
    mailbox: MacosFrameMailbox,
    active: AtomicBool,
    status: Mutex<NativeProtectedSourceState>,
    selection: Mutex<MacosCaptureSelection>,
    captured_at: Mutex<Option<Instant>>,
}

#[cfg(feature = "macos-capture-fixtures")]
impl Default for FixtureControl {
    fn default() -> Self {
        Self {
            mailbox: MacosFrameMailbox::default(),
            active: AtomicBool::new(false),
            status: Mutex::new(NativeProtectedSourceState::ReadyIdle),
            selection: Mutex::new(MacosCaptureSelection::None),
            captured_at: Mutex::new(None),
        }
    }
}

#[cfg(feature = "macos-capture-fixtures")]
impl MacosCaptureControl for FixtureControl {
    fn mailbox(&self) -> MacosFrameMailbox {
        self.mailbox.clone()
    }

    fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Release);
        *lock(&self.status) = if active {
            NativeProtectedSourceState::Starting
        } else {
            NativeProtectedSourceState::ReadyIdle
        };
    }

    fn present_picker(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn request_authorization(&self) -> NativeProtectedSourceState {
        *lock(&self.status) = NativeProtectedSourceState::NeedsSelection;
        NativeProtectedSourceState::NeedsSelection
    }

    fn status(&self) -> NativeProtectedSourceState {
        *lock(&self.status)
    }

    fn selection(&self) -> MacosCaptureSelection {
        lock(&self.selection).clone()
    }

    fn authorization(&self) -> MacosAuthorizationState {
        match self.status() {
            NativeProtectedSourceState::PermissionDenied | NativeProtectedSourceState::Revoked => {
                MacosAuthorizationState::Denied
            }
            NativeProtectedSourceState::NeedsUserAction => MacosAuthorizationState::NotDetermined,
            NativeProtectedSourceState::Disabled => MacosAuthorizationState::Unknown,
            _ => MacosAuthorizationState::Authorized,
        }
    }

    fn captured_at(&self, _display_time: u64) -> anyhow::Result<Instant> {
        Ok(lock(&self.captured_at).take().unwrap_or_else(Instant::now))
    }
}

#[cfg(feature = "macos-capture-fixtures")]
pub struct MacosScreenCaptureFixture {
    control: Arc<FixtureControl>,
}

#[cfg(feature = "macos-capture-fixtures")]
impl MacosScreenCaptureFixture {
    pub fn source(
        config: CaptureConfig,
        admission: ScreenByteAdmissionCoordinator,
    ) -> (MacosScreenCaptureInput, Self) {
        let control = Arc::new(FixtureControl {
            status: Mutex::new(NativeProtectedSourceState::ReadyIdle),
            ..FixtureControl::default()
        });
        let source = MacosScreenCaptureInput::with_control(config, admission, control.clone());
        (source, Self { control })
    }

    pub fn publish(&self, frame: MacosCaptureFrame) {
        *lock(&self.control.status) = NativeProtectedSourceState::Live;
        self.control
            .mailbox
            .publish(Ok(MacosFrameEvent::Frame(Box::new(frame))));
    }

    pub fn publish_at(&self, frame: MacosCaptureFrame, captured_at: Instant) {
        *lock(&self.control.captured_at) = Some(captured_at);
        self.publish(frame);
    }

    pub fn is_active(&self) -> bool {
        self.control.active.load(Ordering::Acquire)
    }

    pub fn set_selection(&self, selection: MacosCaptureSelection) {
        *lock(&self.control.selection) = selection;
    }
}
