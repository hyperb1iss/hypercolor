use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
#[cfg(unix)]
use hypercolor_core::device::{BlocksBackend, BlocksScanner};
use hypercolor_core::device::{
    SmBusBackend, SmBusScanner, UsbBackend, UsbProtocolConfigStore, UsbScanner,
};
use hypercolor_driver_api::{
    DeviceBackend, DeviceBackendFactory, DiscoveredDevice, DiscoveryCapability, DiscoveryRequest,
    DriverConfigView, DriverDescriptor, DriverError, DriverHost, DriverModule, OutputBinding,
};
#[cfg(unix)]
use hypercolor_types::device::BLOCKS_OUTPUT_BACKEND_ID;
use hypercolor_types::device::{
    DriverModuleDescriptor, DriverTransportKind, SMBUS_OUTPUT_BACKEND_ID, USB_OUTPUT_BACKEND_ID,
};
use hypercolor_types::identity::BackendId;

static USB_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    USB_OUTPUT_BACKEND_ID,
    "USB Transport",
    DriverTransportKind::Usb,
    true,
    false,
);

static SMBUS_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    SMBUS_OUTPUT_BACKEND_ID,
    "SMBus Transport",
    DriverTransportKind::Smbus,
    true,
    false,
);

#[cfg(unix)]
static BLOCKS_DESCRIPTOR: DriverDescriptor = DriverDescriptor::new(
    BLOCKS_OUTPUT_BACKEND_ID,
    "ROLI Blocks Bridge",
    DriverTransportKind::Bridge,
    true,
    false,
);

pub struct UsbTransportDriverModule {
    protocol_configs: UsbProtocolConfigStore,
}

impl UsbTransportDriverModule {
    pub fn new(protocol_configs: UsbProtocolConfigStore) -> Self {
        Self { protocol_configs }
    }
}

impl DriverModule for UsbTransportDriverModule {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &USB_DESCRIPTOR
    }

    fn module_descriptor(&self) -> DriverModuleDescriptor {
        let mut descriptor = self.descriptor().module_descriptor();
        descriptor.default_enabled = false;
        descriptor
    }

    fn output(&self) -> OutputBinding<'_> {
        OutputBinding::Owned {
            id: BackendId::new(USB_OUTPUT_BACKEND_ID).expect("USB backend ID must be valid"),
            factory: self,
        }
    }

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        Some(self)
    }
}

impl DeviceBackendFactory for UsbTransportDriverModule {
    fn build(
        &self,
        _host: &dyn DriverHost,
        _config: DriverConfigView<'_>,
    ) -> Result<Arc<dyn DeviceBackend>, DriverError> {
        Ok(Arc::new(UsbBackend::with_protocol_config_store(
            self.protocol_configs.clone(),
        )))
    }
}

#[async_trait]
impl DiscoveryCapability for UsbTransportDriverModule {
    async fn discover(
        &self,
        _host: &dyn DriverHost,
        _request: &DiscoveryRequest,
        _config: DriverConfigView<'_>,
    ) -> Result<Vec<DiscoveredDevice>, DriverError> {
        UsbScanner::new()
            .scan()
            .await
            .map_err(DriverError::discovery)
    }
}

pub struct SmBusTransportDriverModule;

impl DriverModule for SmBusTransportDriverModule {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &SMBUS_DESCRIPTOR
    }

    fn module_descriptor(&self) -> DriverModuleDescriptor {
        let mut descriptor = self.descriptor().module_descriptor();
        descriptor.default_enabled = false;
        descriptor
    }

    fn output(&self) -> OutputBinding<'_> {
        OutputBinding::Owned {
            id: BackendId::new(SMBUS_OUTPUT_BACKEND_ID).expect("SMBus backend ID must be valid"),
            factory: self,
        }
    }

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        Some(self)
    }
}

impl DeviceBackendFactory for SmBusTransportDriverModule {
    fn build(
        &self,
        _host: &dyn DriverHost,
        _config: DriverConfigView<'_>,
    ) -> Result<Arc<dyn DeviceBackend>, DriverError> {
        Ok(Arc::new(SmBusBackend::new()))
    }
}

#[async_trait]
impl DiscoveryCapability for SmBusTransportDriverModule {
    async fn discover(
        &self,
        _host: &dyn DriverHost,
        _request: &DiscoveryRequest,
        _config: DriverConfigView<'_>,
    ) -> Result<Vec<DiscoveredDevice>, DriverError> {
        SmBusScanner::new()
            .scan()
            .await
            .map_err(DriverError::discovery)
    }
}

#[cfg(unix)]
pub struct BlocksTransportDriverModule {
    socket_path: PathBuf,
    default_enabled: bool,
}

#[cfg(unix)]
impl BlocksTransportDriverModule {
    pub fn new(socket_path: PathBuf, default_enabled: bool) -> Self {
        Self {
            socket_path,
            default_enabled,
        }
    }
}

#[cfg(unix)]
impl DriverModule for BlocksTransportDriverModule {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &BLOCKS_DESCRIPTOR
    }

    fn module_descriptor(&self) -> DriverModuleDescriptor {
        let mut descriptor = self.descriptor().module_descriptor();
        descriptor.default_enabled = self.default_enabled;
        descriptor
    }

    fn output(&self) -> OutputBinding<'_> {
        OutputBinding::Owned {
            id: BackendId::new(BLOCKS_OUTPUT_BACKEND_ID).expect("Blocks backend ID must be valid"),
            factory: self,
        }
    }

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        Some(self)
    }
}

#[cfg(unix)]
impl DeviceBackendFactory for BlocksTransportDriverModule {
    fn build(
        &self,
        _host: &dyn DriverHost,
        _config: DriverConfigView<'_>,
    ) -> Result<Arc<dyn DeviceBackend>, DriverError> {
        Ok(Arc::new(BlocksBackend::new(self.socket_path.clone())))
    }
}

#[cfg(unix)]
#[async_trait]
impl DiscoveryCapability for BlocksTransportDriverModule {
    async fn discover(
        &self,
        _host: &dyn DriverHost,
        _request: &DiscoveryRequest,
        _config: DriverConfigView<'_>,
    ) -> Result<Vec<DiscoveredDevice>, DriverError> {
        BlocksScanner::new(self.socket_path.clone())
            .scan()
            .await
            .map_err(DriverError::discovery)
    }
}
