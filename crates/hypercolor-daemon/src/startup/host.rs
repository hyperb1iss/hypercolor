//! What a daemon reaches on the machine it runs on.

use std::sync::Arc;

use anyhow::Result;
use hypercolor_core::config::ConfigManager;
use hypercolor_core::config::paths::IsolatedUserDirs;
use hypercolor_core::device::usb_hotplug::UsbHotplugTask;
use hypercolor_core::device::{UnclaimedDeviceStore, UsbHotplugMonitor, UsbProtocolConfigStore};
use hypercolor_core::input::InputManager;
use hypercolor_core::input::screen::ScreenMonitor;
use hypercolor_core::session::SessionMonitor;
use hypercolor_driver_support::CredentialStore;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::api::system::AudioDeviceInfo;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::host_software::{HardwareBrokerReport, HostInventory, HostSoftwareSnapshot};

use super::services::{ScreenCapacityPlan, compose_native_inputs, compose_silent_inputs};
use crate::api::system::enumerate_audio_input_devices;
use crate::network;

pub(super) type BuildDrivers = fn(
    &HypercolorConfig,
    Arc<CredentialStore>,
    UsbProtocolConfigStore,
    UnclaimedDeviceStore,
) -> Result<DriverModuleRegistry>;

pub(super) type ComposeInputs =
    fn(&HypercolorConfig, &Arc<ConfigManager>, &InputManager, ScreenCapacityPlan) -> Result<()>;

/// What a running daemon keeps reaching for on its host: the readings it
/// takes on request.
#[derive(Clone, Copy)]
pub(crate) struct HostCapabilities {
    pub(crate) running_software: fn() -> HostInventory,
    pub(crate) hardware_broker: fn() -> Option<HardwareBrokerReport>,
    pub(crate) audio_devices: fn() -> Result<Vec<AudioDeviceInfo>>,
    pub(crate) capture_monitors: fn() -> Vec<ScreenMonitor>,
    pub(crate) usb_hotplug: fn(&UsbHotplugMonitor) -> Result<Option<UsbHotplugTask>>,
}

impl HostCapabilities {
    /// The machine the daemon runs on.
    pub(crate) const NATIVE: Self = Self {
        running_software: crate::session::host_inventory,
        hardware_broker: crate::session::hardware_broker_report,
        audio_devices: enumerate_audio_input_devices,
        capture_monitors: hypercolor_core::input::screen::available_monitors,
        usb_hotplug: |monitor| monitor.start().map(Some),
    };

    /// A host with nothing on it: every reading is empty.
    pub(crate) const ISOLATED: Self = Self {
        running_software: || HostInventory::Listed(HostSoftwareSnapshot::default()),
        hardware_broker: || None,
        audio_devices: || Ok(Vec::new()),
        capture_monitors: Vec::new,
        usb_hotplug: |_| Ok(None),
    };
}

/// The daemon's ports onto its host, chosen once at the composition root.
///
/// Discovery transports, input sources, session monitors, and every host
/// reading all reach outside the process. Initialization takes them as one
/// required value, so nothing below the composition root decides what the
/// daemon may touch.
pub struct HostPorts {
    pub(super) build_drivers: BuildDrivers,
    pub(super) compose_inputs: ComposeInputs,
    pub(super) session_monitors: Option<Vec<Box<dyn SessionMonitor>>>,
    pub(super) capabilities: HostCapabilities,
    pub(super) user_dirs: Option<Arc<IsolatedUserDirs>>,
}

impl HostPorts {
    /// The real host: every driver scans, native inputs run, and the
    /// platform's session monitors watch power and login state.
    #[must_use]
    pub fn native() -> Self {
        Self {
            build_drivers: network::build_builtin_driver_module_registry,
            compose_inputs: compose_native_inputs,
            session_monitors: None,
            capabilities: HostCapabilities::NATIVE,
            user_dirs: None,
        }
    }

    /// Replace the platform's session monitors, when the process host
    /// supplies its own (a Windows service receives power events itself).
    #[must_use]
    pub fn with_session_monitors(mut self, monitors: Vec<Box<dyn SessionMonitor>>) -> Self {
        self.session_monitors = Some(monitors);
        self
    }

    /// A daemon that touches nothing outside `dirs`: drivers compose but
    /// never scan, no input source reads the host, no monitor watches the
    /// session, and every host reading comes back empty. Audio stays
    /// registered on a source with no device.
    ///
    /// The ports keep a share of the claim, and so does the daemon composed
    /// from them, so every user directory stays inside it while either
    /// lives.
    #[must_use]
    pub fn isolated(dirs: &Arc<IsolatedUserDirs>) -> Self {
        Self {
            build_drivers: network::build_offline_driver_module_registry,
            compose_inputs: compose_silent_inputs,
            session_monitors: Some(Vec::new()),
            capabilities: HostCapabilities::ISOLATED,
            user_dirs: Some(Arc::clone(dirs)),
        }
    }
}
