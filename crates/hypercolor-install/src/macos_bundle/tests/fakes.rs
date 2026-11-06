//! An in-memory installation whose every effect is a numbered boundary a
//! test can crash at.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::macos_bundle::{
    MACOS_BUNDLE_DISCARD_SUFFIX, MACOS_BUNDLE_PARTIAL_SUFFIX, MacosBundleExecutor,
    MacosBundleIdentity, MacosBundleMaterialization, MacosBundleObservation,
    MacosBundleRunningDaemon, MacosBundleService, MacosBundleServiceObservation, MacosBundleSlot,
    MacosBundleUnit,
};
use crate::{InstallPlatformError, UnitRecord};

/// The unwinding payload of a simulated crash or power loss.
#[derive(Debug)]
pub(super) struct Crash;

/// Numbered effect boundaries, shared by every fake and wrapper in a test.
#[derive(Debug, Default)]
pub(super) struct Boundaries {
    pub(super) passed: usize,
    pub(super) crash_at: Option<usize>,
    pub(super) log: Vec<String>,
}

pub(super) type SharedBoundaries = Arc<Mutex<Boundaries>>;

pub(super) fn lock<T>(shared: &Mutex<T>) -> MutexGuard<'_, T> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Run `body`, turning a simulated crash into `Err(Crash)`.
pub(super) fn survive<R>(body: impl FnOnce() -> R) -> Result<R, Crash> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(result) => Ok(result),
        Err(payload) => match payload.downcast::<Crash>() {
            Ok(_) => Err(Crash),
            Err(payload) => std::panic::resume_unwind(payload),
        },
    }
}

/// Pass one boundary, crashing there when the test asked for it.
pub(super) fn boundary(boundaries: &SharedBoundaries, label: &str) {
    let crash = {
        let mut boundaries = lock(boundaries);
        boundaries.passed += 1;
        boundaries.log.push(label.to_owned());
        boundaries.crash_at == Some(boundaries.passed)
    };
    if crash {
        std::panic::resume_unwind(Box::new(Crash));
    }
}

/// What the fake installation holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct World {
    pub(super) installed: MacosBundleSlot,
    pub(super) staged: BTreeMap<String, MacosBundleSlot>,
    pub(super) interrupted: BTreeSet<String>,
    pub(super) available_bytes: u64,
    pub(super) registered: bool,
    pub(super) enabled: bool,
    pub(super) running: Option<MacosBundleRunningDaemon>,
    pub(super) next_pid: u32,
    /// Proofs of a daemon with this cdhash fail.
    pub(super) failing_proof: Option<String>,
    pub(super) installed_daemon: PathBuf,
}

impl World {
    /// An installation running `installed`'s daemon from its bundle.
    pub(super) fn running(installed: &MacosBundleIdentity, installed_daemon: PathBuf) -> Self {
        Self {
            installed: MacosBundleSlot::Bundle(installed.clone()),
            staged: BTreeMap::new(),
            interrupted: BTreeSet::new(),
            available_bytes: u64::MAX,
            registered: true,
            enabled: true,
            running: Some(MacosBundleRunningDaemon {
                pid: 100,
                executable: installed_daemon.clone(),
                cdhash: installed.daemon.clone(),
            }),
            next_pid: 101,
            failing_proof: None,
            installed_daemon,
        }
    }

    /// The service manager starts the installed bundle's daemon on its own,
    /// as launchd does at login.
    pub(super) fn autostart(&mut self) {
        let MacosBundleSlot::Bundle(identity) = &self.installed else {
            panic!("nothing installed to autostart");
        };
        self.running = Some(MacosBundleRunningDaemon {
            pid: self.next_pid,
            executable: self.installed_daemon.clone(),
            cdhash: identity.daemon.clone(),
        });
        self.next_pid += 1;
    }
}

pub(super) type SharedWorld = Arc<Mutex<World>>;

fn fail(detail: &str) -> InstallPlatformError {
    InstallPlatformError::new(detail)
}

pub(super) struct FakeExecutor {
    pub(super) world: SharedWorld,
    pub(super) boundaries: SharedBoundaries,
}

impl MacosBundleExecutor for FakeExecutor {
    fn observe(&mut self) -> Result<MacosBundleObservation, InstallPlatformError> {
        let world = lock(&self.world);
        Ok(MacosBundleObservation {
            installed: world.installed.clone(),
            staged: world.staged.clone(),
            interrupted: world.interrupted.clone(),
        })
    }

    fn available_bytes(&mut self) -> Result<u64, InstallPlatformError> {
        Ok(lock(&self.world).available_bytes)
    }

    fn materialize(
        &mut self,
        _source: &UnitRecord,
        unit: &MacosBundleUnit,
        name: &str,
    ) -> Result<MacosBundleMaterialization, InstallPlatformError> {
        boundary(&self.boundaries, "materialize:before");
        let partial = format!("{name}{MACOS_BUNDLE_PARTIAL_SUFFIX}");
        {
            let mut world = lock(&self.world);
            if world.staged.contains_key(name) {
                return Err(fail("staged copy exists"));
            }
            world
                .interrupted
                .retain(|leftover| !leftover.starts_with(name));
            world.interrupted.insert(partial.clone());
        }
        boundary(&self.boundaries, "materialize:partial");
        {
            let mut world = lock(&self.world);
            world.interrupted.remove(&partial);
            world.staged.insert(
                name.to_owned(),
                MacosBundleSlot::Bundle(unit.identity().clone()),
            );
        }
        boundary(&self.boundaries, "materialize:after");
        Ok(MacosBundleMaterialization {
            cloned_files: 1,
            copied_files: 0,
        })
    }

    fn exchange(&mut self, name: &str) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "exchange:before");
        {
            let mut world = lock(&self.world);
            let staged = world
                .staged
                .get(name)
                .cloned()
                .filter(|slot| !slot.is_absent())
                .ok_or_else(|| fail("nothing staged to exchange"))?;
            if world.installed.is_absent() {
                return Err(fail("nothing installed to exchange"));
            }
            let installed = std::mem::replace(&mut world.installed, staged);
            world.staged.insert(name.to_owned(), installed);
        }
        boundary(&self.boundaries, "exchange:after");
        Ok(())
    }

    fn install_staged(&mut self, name: &str) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "install:before");
        {
            let mut world = lock(&self.world);
            if !world.installed.is_absent() {
                return Err(fail("something is installed"));
            }
            let staged = world
                .staged
                .insert(name.to_owned(), MacosBundleSlot::Absent)
                .ok_or_else(|| fail("nothing staged to install"))?;
            world.installed = staged;
        }
        boundary(&self.boundaries, "install:after");
        Ok(())
    }

    fn withdraw_installed(&mut self, name: &str) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "withdraw:before");
        {
            let mut world = lock(&self.world);
            if !world
                .staged
                .get(name)
                .is_some_and(MacosBundleSlot::is_absent)
            {
                return Err(fail("no empty staging name to withdraw into"));
            }
            let installed = std::mem::replace(&mut world.installed, MacosBundleSlot::Absent);
            world.staged.insert(name.to_owned(), installed);
        }
        boundary(&self.boundaries, "withdraw:after");
        Ok(())
    }

    fn discard(&mut self, name: &str) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "discard:before");
        let discard = format!("{name}{MACOS_BUNDLE_DISCARD_SUFFIX}");
        let renamed = {
            let mut world = lock(&self.world);
            world
                .interrupted
                .remove(&format!("{name}{MACOS_BUNDLE_PARTIAL_SUFFIX}"));
            let renamed = world.staged.remove(name).is_some();
            if renamed {
                world.interrupted.insert(discard.clone());
            }
            renamed
        };
        if renamed {
            boundary(&self.boundaries, "discard:renamed");
        }
        lock(&self.world).interrupted.remove(&discard);
        boundary(&self.boundaries, "discard:after");
        Ok(())
    }
}

pub(super) struct FakeService {
    pub(super) world: SharedWorld,
    pub(super) boundaries: SharedBoundaries,
}

impl MacosBundleService for FakeService {
    fn observe(&mut self) -> Result<MacosBundleServiceObservation, InstallPlatformError> {
        let world = lock(&self.world);
        Ok(MacosBundleServiceObservation {
            registered: world.registered,
            enabled: world.enabled,
            running: world.running.clone(),
        })
    }

    fn set_enabled(&mut self, enabled: bool) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "enable:before");
        lock(&self.world).enabled = enabled;
        boundary(&self.boundaries, "enable:after");
        Ok(())
    }

    fn start(&mut self) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "start:before");
        lock(&self.world).autostart();
        boundary(&self.boundaries, "start:after");
        Ok(())
    }

    fn stop(&mut self, daemon: &MacosBundleRunningDaemon) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "stop:before");
        {
            let mut world = lock(&self.world);
            if world.running.as_ref() != Some(daemon) {
                return Err(fail("stop names another daemon"));
            }
            world.running = None;
        }
        boundary(&self.boundaries, "stop:after");
        Ok(())
    }

    fn wait_for_release(&mut self) -> Result<(), InstallPlatformError> {
        Ok(())
    }

    fn prove(
        &mut self,
        daemon: &MacosBundleRunningDaemon,
        _version: &str,
    ) -> Result<(), InstallPlatformError> {
        boundary(&self.boundaries, "prove");
        if lock(&self.world).failing_proof.as_deref() == Some(daemon.cdhash.as_str()) {
            return Err(fail("the daemon never became ready"));
        }
        Ok(())
    }
}
