use anyhow::Result;
use hypercolor_driver_api::{
    DeviceBackend, DriverConfigView, DriverDescriptor, DriverHost, DriverModule,
    DriverProtocolCatalog, DriverTransport,
};
use hypercolor_hal::ProtocolDatabase;
use hypercolor_types::device::{
    DriverModuleDescriptor, DriverProtocolDescriptor, DriverTransportKind,
};

pub struct HalCatalogDriverModule {
    descriptor: &'static DriverDescriptor,
    module_descriptor: DriverModuleDescriptor,
    protocols: Vec<DriverProtocolDescriptor>,
}

impl HalCatalogDriverModule {
    fn new(
        module_descriptor: DriverModuleDescriptor,
        protocols: Vec<DriverProtocolDescriptor>,
    ) -> Self {
        let transport = module_descriptor
            .transports
            .first()
            .map_or(DriverTransport::Usb, driver_transport);
        let descriptor = DriverDescriptor::new(
            leak_string(module_descriptor.id.clone()),
            leak_string(module_descriptor.display_name.clone()),
            transport,
            false,
            false,
        );
        let descriptor = Box::leak(Box::new(descriptor));

        Self {
            descriptor,
            module_descriptor,
            protocols,
        }
    }
}

impl DriverProtocolCatalog for HalCatalogDriverModule {
    fn descriptors(&self) -> &[DriverProtocolDescriptor] {
        &self.protocols
    }
}

impl DriverModule for HalCatalogDriverModule {
    fn descriptor(&self) -> &'static DriverDescriptor {
        self.descriptor
    }

    fn module_descriptor(&self) -> DriverModuleDescriptor {
        self.module_descriptor.clone()
    }

    fn build_output_backend(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> Result<Option<Box<dyn DeviceBackend>>> {
        let _ = (host, config);
        Ok(None)
    }

    fn has_output_backend(&self) -> bool {
        false
    }

    fn protocol_catalog(&self) -> Option<&dyn DriverProtocolCatalog> {
        Some(self)
    }
}

pub fn hal_catalog_driver_modules() -> Vec<HalCatalogDriverModule> {
    hal_module_descriptors()
        .iter()
        .cloned()
        .map(|module_descriptor| {
            let protocols =
                ProtocolDatabase::protocol_descriptors_for_driver(&module_descriptor.id);
            HalCatalogDriverModule::new(module_descriptor, protocols)
        })
        .collect()
}

pub fn hal_module_descriptors() -> &'static [DriverModuleDescriptor] {
    ProtocolDatabase::module_descriptors()
}

fn leak_string(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

const fn driver_transport(transport: &DriverTransportKind) -> DriverTransport {
    match transport {
        DriverTransportKind::Network => DriverTransport::Network,
        DriverTransportKind::Usb => DriverTransport::Usb,
        DriverTransportKind::Smbus => DriverTransport::Smbus,
        DriverTransportKind::Midi => DriverTransport::Midi,
        DriverTransportKind::Serial => DriverTransport::Serial,
        DriverTransportKind::Bridge => DriverTransport::Bridge,
        DriverTransportKind::Virtual => DriverTransport::Virtual,
        DriverTransportKind::Custom(_) => DriverTransport::Virtual,
    }
}
