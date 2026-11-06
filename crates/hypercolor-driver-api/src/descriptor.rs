use hypercolor_types::device::{
    DRIVER_MODULE_API_SCHEMA_VERSION, DriverCapabilitySet, DriverModuleDescriptor, DriverStability,
    DriverTransportDescriptor, DriverTransportKind,
};

/// Current driver API schema version. Bump this on any breaking change to
/// the [`crate::DriverHost`] trait, [`DriverDescriptor`] fields, or related types.
pub const DRIVER_API_SCHEMA_VERSION: u32 = DRIVER_MODULE_API_SCHEMA_VERSION;

/// Static metadata about a modular driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverDescriptor {
    /// Stable machine-readable ID, for example `acme-light-strip`.
    pub id: &'static str,
    /// Human-readable driver name for logs and UI.
    pub display_name: &'static str,
    /// Transport class used by this driver.
    pub transport: DriverTransportKind,
    /// Whether the driver contributes discovery support.
    pub supports_discovery: bool,
    /// Whether the driver contributes pairing support.
    pub supports_pairing: bool,
    /// Schema version of the driver API contract this driver implements.
    /// The host rejects load if this does not match [`DRIVER_API_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Release label; set it with [`DriverDescriptor::experimental`].
    pub stability: DriverStability,
    /// Whether a fresh install runs this driver; clear it with
    /// [`DriverDescriptor::default_disabled`].
    pub default_enabled: bool,
}

impl DriverDescriptor {
    /// Create a new static descriptor tagged with the current
    /// [`DRIVER_API_SCHEMA_VERSION`].
    #[must_use]
    pub const fn new(
        id: &'static str,
        display_name: &'static str,
        transport: DriverTransportKind,
        supports_discovery: bool,
        supports_pairing: bool,
    ) -> Self {
        Self::with_schema_version(
            id,
            display_name,
            transport,
            supports_discovery,
            supports_pairing,
            DRIVER_API_SCHEMA_VERSION,
        )
    }

    /// Create a new static descriptor with an explicit schema version.
    ///
    /// Out-of-tree drivers should prefer [`DriverDescriptor::new`] so they
    /// automatically pick up the current schema version at compile time.
    /// This constructor exists so the host can synthesise descriptors at
    /// other versions in tests and version-mismatch error paths.
    #[must_use]
    pub const fn with_schema_version(
        id: &'static str,
        display_name: &'static str,
        transport: DriverTransportKind,
        supports_discovery: bool,
        supports_pairing: bool,
        schema_version: u32,
    ) -> Self {
        Self {
            id,
            display_name,
            transport,
            supports_discovery,
            supports_pairing,
            schema_version,
            stability: DriverStability::Stable,
            default_enabled: true,
        }
    }

    /// Label this driver experimental: nobody has run it on real hardware
    /// yet (Spec 89). The label changes nothing at runtime; the driver is
    /// enabled and claims devices like any other.
    ///
    /// Usable in a `static` initializer:
    /// `DriverDescriptor::new("lifx", "LIFX", DriverTransportKind::Network, true, false).experimental()`.
    #[must_use]
    pub const fn experimental(mut self) -> Self {
        self.stability = DriverStability::Experimental;
        self
    }

    /// Start this driver disabled until a user turns it on.
    #[must_use]
    pub const fn default_disabled(mut self) -> Self {
        self.default_enabled = false;
        self
    }

    /// Whether this driver carries the experimental label.
    #[must_use]
    pub const fn is_experimental(&self) -> bool {
        self.stability.is_experimental()
    }

    /// Convert this driver-facing descriptor into the host-wide module
    /// descriptor used by registry introspection.
    #[must_use]
    pub fn module_descriptor(&self) -> DriverModuleDescriptor {
        DriverModuleDescriptor {
            id: self.id.to_owned(),
            display_name: self.display_name.to_owned(),
            vendor_name: None,
            module_kind: self.transport.module_kind(),
            transports: vec![DriverTransportDescriptor::available(self.transport.clone())],
            capabilities: DriverCapabilitySet {
                config: false,
                discovery: self.supports_discovery,
                pairing: self.supports_pairing,
                output_backend: true,
                protocol_catalog: false,
                runtime_cache: false,
                credentials: self.supports_pairing,
                presentation: false,
                controls: false,
            },
            api_schema_version: self.schema_version,
            config_version: 1,
            default_enabled: self.default_enabled,
            stability: self.stability,
        }
    }
}
