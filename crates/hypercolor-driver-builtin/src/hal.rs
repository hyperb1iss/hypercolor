use hypercolor_driver_api::{DriverDescriptor, DriverModule, DriverProtocolCatalog, OutputBinding};
use hypercolor_hal::ProtocolDatabase;
use hypercolor_types::device::{
    DriverModuleDescriptor, DriverProtocolDescriptor, DriverTransportKind, SMBUS_OUTPUT_BACKEND_ID,
    USB_OUTPUT_BACKEND_ID,
};
use hypercolor_types::identity::BackendId;

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
            .cloned()
            .unwrap_or(DriverTransportKind::Usb);
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

    fn output(&self) -> OutputBinding<'_> {
        if self
            .module_descriptor
            .transports
            .contains(&DriverTransportKind::Smbus)
        {
            return OutputBinding::Shared(
                BackendId::new(SMBUS_OUTPUT_BACKEND_ID).expect("SMBus backend ID must be valid"),
            );
        }
        if self.module_descriptor.transports.iter().any(|transport| {
            matches!(
                transport,
                DriverTransportKind::Usb | DriverTransportKind::Midi | DriverTransportKind::Serial
            )
        }) {
            return OutputBinding::Shared(
                BackendId::new(USB_OUTPUT_BACKEND_ID).expect("USB backend ID must be valid"),
            );
        }
        OutputBinding::None
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
