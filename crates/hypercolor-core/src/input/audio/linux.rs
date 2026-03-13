//! Linux PulseAudio / PipeWire source discovery helpers.

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Context as _, anyhow};
use libpulse_binding as pulse;
use pulse::callbacks::ListResult;
use pulse::context::introspect::ServerInfo;
use pulse::context::{Context, FlagSet as ContextFlagSet, State as ContextState};
use pulse::mainloop::standard::{IterateResult, Mainloop};
use pulse::operation::{Operation, State as OperationState};

/// Linux PulseAudio / PipeWire source snapshot used for deterministic tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PulseSourceSnapshot {
    /// Pulse source name, e.g. `alsa_output.pci-0000_00_1f.3.analog-stereo.monitor`.
    pub name: String,
    /// Human-readable source description when available.
    pub description: Option<String>,
    /// Owning sink name for monitor sources.
    pub monitor_of_sink_name: Option<String>,
}

impl PulseSourceSnapshot {
    #[must_use]
    pub fn is_monitor(&self) -> bool {
        self.monitor_of_sink_name.is_some() || self.name.ends_with(".monitor")
    }
}

/// Named Linux audio source surfaced to the daemon settings UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinuxNamedAudioSource {
    /// Stable source ID persisted in config.
    pub id: String,
    /// User-facing display name.
    pub name: String,
    /// Explanatory detail shown in the settings UI.
    pub description: String,
    /// Whether this source is a system-output monitor.
    pub is_monitor: bool,
    /// Whether this source mirrors the current default output.
    pub is_default_monitor: bool,
}

/// Enumerate Linux PipeWire / PulseAudio sources suitable for named selection.
///
/// # Errors
///
/// Returns an error if the PulseAudio compatibility server cannot be queried.
pub fn enumerate_named_audio_sources() -> anyhow::Result<Vec<LinuxNamedAudioSource>> {
    let pulse_state = query_pulse_state()?;
    Ok(build_named_audio_sources(
        &pulse_state.sources,
        pulse_state.default_sink_name.as_deref(),
    ))
}

/// Resolve the current default output monitor source name.
///
/// # Errors
///
/// Returns an error if the PulseAudio compatibility server cannot be queried.
pub fn default_monitor_source_name() -> anyhow::Result<Option<String>> {
    let pulse_state = query_pulse_state()?;
    Ok(default_monitor_source_name_from_snapshots(
        &pulse_state.sources,
        pulse_state.default_sink_name.as_deref(),
    ))
}

/// Check whether a PulseAudio / PipeWire source exists by exact name.
///
/// # Errors
///
/// Returns an error if the PulseAudio compatibility server cannot be queried.
pub fn pulse_source_exists(source_name: &str) -> anyhow::Result<bool> {
    let wanted = source_name.trim();
    if wanted.is_empty() {
        return Ok(false);
    }

    let pulse_state = query_pulse_state()?;
    Ok(pulse_state
        .sources
        .iter()
        .any(|source| source.name.eq_ignore_ascii_case(wanted)))
}

/// Pure helper used by tests and the daemon settings layer.
#[must_use]
pub fn build_named_audio_sources(
    sources: &[PulseSourceSnapshot],
    default_sink_name: Option<&str>,
) -> Vec<LinuxNamedAudioSource> {
    let default_monitor_name =
        default_monitor_source_name_from_snapshots(sources, default_sink_name);
    let mut named_sources: Vec<_> = sources
        .iter()
        .filter_map(|source| {
            let id = source.name.trim();
            if id.is_empty() {
                return None;
            }

            let is_monitor = source.is_monitor();
            let is_default_monitor = default_monitor_name
                .as_deref()
                .is_some_and(|default_name| default_name == id);
            let display_name = source
                .description
                .clone()
                .filter(|description| !description.trim().is_empty())
                .unwrap_or_else(|| id.to_owned());

            let description = if is_monitor {
                if is_default_monitor {
                    "Capture the active default system output".to_owned()
                } else {
                    format!("Capture audio playing on this output ({id})")
                }
            } else {
                format!("Capture from this input source ({id})")
            };

            Some(LinuxNamedAudioSource {
                id: id.to_owned(),
                name: display_name,
                description,
                is_monitor,
                is_default_monitor,
            })
        })
        .collect();

    named_sources.sort_by_cached_key(|source| {
        let class_rank = if source.is_default_monitor {
            0
        } else if source.is_monitor {
            1
        } else {
            2
        };
        (class_rank, source.name.to_ascii_lowercase())
    });

    named_sources
}

/// Pure helper used by tests and monitor auto-selection.
#[must_use]
pub fn default_monitor_source_name_from_snapshots(
    sources: &[PulseSourceSnapshot],
    default_sink_name: Option<&str>,
) -> Option<String> {
    let default_sink_name = default_sink_name?.trim();
    if default_sink_name.is_empty() {
        return None;
    }

    sources
        .iter()
        .find(|source| {
            source
                .monitor_of_sink_name
                .as_deref()
                .is_some_and(|sink_name| sink_name == default_sink_name)
        })
        .or_else(|| {
            let monitor_name = format!("{default_sink_name}.monitor");
            sources.iter().find(|source| source.name == monitor_name)
        })
        .map(|source| source.name.clone())
}

#[derive(Debug)]
struct PulseStateSnapshot {
    default_sink_name: Option<String>,
    sources: Vec<PulseSourceSnapshot>,
}

struct PulseSession {
    context: Context,
    mainloop: Mainloop,
}

impl PulseSession {
    fn connect() -> anyhow::Result<Self> {
        let mainloop =
            Mainloop::new().ok_or_else(|| anyhow!("failed to create PulseAudio mainloop"))?;
        let mut context = Context::new(&mainloop, "hypercolor-audio")
            .ok_or_else(|| anyhow!("failed to create PulseAudio context"))?;
        context
            .connect(None, ContextFlagSet::NOFLAGS, None)
            .map_err(|error| {
                anyhow!("failed to connect to PulseAudio compatibility server: {error:?}")
            })?;

        let mut session = Self { context, mainloop };
        session.wait_for_context_ready()?;
        Ok(session)
    }

    fn wait_for_context_ready(&mut self) -> anyhow::Result<()> {
        loop {
            match self.context.get_state() {
                ContextState::Ready => return Ok(()),
                ContextState::Failed | ContextState::Terminated => {
                    return Err(anyhow!(
                        "PulseAudio context failed before becoming ready: {:?}",
                        self.context.errno()
                    ));
                }
                _ => self.iterate(true)?,
            }
        }
    }

    fn iterate(&mut self, block: bool) -> anyhow::Result<()> {
        match self.mainloop.iterate(block) {
            IterateResult::Success(_) => Ok(()),
            IterateResult::Quit(retval) => Err(anyhow!(
                "PulseAudio mainloop quit unexpectedly with status {}",
                retval.0
            )),
            IterateResult::Err(error) => {
                Err(anyhow!("PulseAudio mainloop iteration failed: {error:?}"))
            }
        }
    }

    fn wait_for_operation<ClosureProto: ?Sized>(
        &mut self,
        operation: &Operation<ClosureProto>,
        done: impl Fn() -> bool,
    ) -> anyhow::Result<()> {
        loop {
            if done() {
                return Ok(());
            }

            match operation.get_state() {
                OperationState::Done => return Ok(()),
                OperationState::Cancelled => {
                    return Err(anyhow!("PulseAudio operation was cancelled"));
                }
                _ => self.iterate(true)?,
            }
        }
    }

    fn server_info(&mut self) -> anyhow::Result<ServerInfoSnapshot> {
        let server_info = Rc::new(RefCell::new(None));
        let result = Rc::clone(&server_info);
        let operation = self
            .context
            .introspect()
            .get_server_info(move |info: &ServerInfo| {
                *result.borrow_mut() = Some(ServerInfoSnapshot {
                    default_sink_name: info.default_sink_name.as_deref().map(str::to_owned),
                });
            });
        self.wait_for_operation(&operation, || server_info.borrow().is_some())?;
        server_info
            .borrow_mut()
            .take()
            .context("PulseAudio server info callback did not return any data")
    }

    fn source_snapshots(&mut self) -> anyhow::Result<Vec<PulseSourceSnapshot>> {
        let snapshots = Rc::new(RefCell::new(Vec::new()));
        let finished = Rc::new(RefCell::new(false));
        let result = Rc::clone(&snapshots);
        let done = Rc::clone(&finished);
        let operation = self
            .context
            .introspect()
            .get_source_info_list(move |entry| match entry {
                ListResult::Item(source) => {
                    let Some(name) = source.name.as_deref().map(str::to_owned) else {
                        return;
                    };
                    result.borrow_mut().push(PulseSourceSnapshot {
                        name,
                        description: source.description.as_deref().map(str::to_owned),
                        monitor_of_sink_name: source
                            .monitor_of_sink_name
                            .as_deref()
                            .map(str::to_owned),
                    });
                }
                ListResult::End | ListResult::Error => {
                    *done.borrow_mut() = true;
                }
            });
        self.wait_for_operation(&operation, || *finished.borrow())?;
        Ok(snapshots.borrow().clone())
    }
}

impl Drop for PulseSession {
    fn drop(&mut self) {
        self.context.disconnect();
    }
}

#[derive(Debug)]
struct ServerInfoSnapshot {
    default_sink_name: Option<String>,
}

fn query_pulse_state() -> anyhow::Result<PulseStateSnapshot> {
    let mut session = PulseSession::connect()?;
    let server_info = session.server_info()?;
    let sources = session.source_snapshots()?;
    Ok(PulseStateSnapshot {
        default_sink_name: server_info.default_sink_name,
        sources,
    })
}
