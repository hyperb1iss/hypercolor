#![allow(clippy::needless_for_each)]

use utoipa::openapi::path::OperationBuilder;
use utoipa::openapi::path::{Parameter, ParameterBuilder, ParameterIn};
use utoipa::openapi::request_body::RequestBodyBuilder;
use utoipa::openapi::schema::{ObjectBuilder, Type};
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{Content, HttpMethod, Ref, Required, Response, Tag};
use utoipa::{Modify, OpenApi};
use utoipa_swagger_ui::SwaggerUi;

use crate::api::{
    capture, config, controls, devices, drivers, effects, envelope, layers, output, profiles,
    scenes_zones, system,
};

#[derive(OpenApi)]
#[openapi(
    paths(
        system::health_check,
        system::get_server,
        system::get_status,
        capture::authorize_input_monitoring,
        capture::authorize_screen_recording,
        capture::pick_capture_source,
        capture::list_capture_monitors,
        drivers::list_drivers,
        drivers::get_driver_config,
        devices::list_devices,
        devices::get_device,
        effects::list_effects,
        effects::get_effect,
        effects::get_active_effect,
        effects::list_effect_presets,
        effects::apply_effect,
        effects::apply_effect_preset,
        output::get_output,
        output::patch_output,
    ),
    components(
        schemas(
            envelope::Meta,
            hypercolor_types::api::envelope::ApiErrorDetail,
            hypercolor_types::api::envelope::ApiErrorBody,
            envelope::ApiResponse<system::SystemStatus>,
            envelope::ApiResponse<system::ServerInfo>,
            envelope::ApiResponse<hypercolor_types::api::capture::CaptureAuthorizationResponse>,
            envelope::ApiResponse<hypercolor_types::api::capture::CapturePickerResponse>,
            envelope::ApiResponse<Vec<hypercolor_types::api::capture::CaptureMonitor>>,
            envelope::ApiResponse<drivers::DriverListResponse>,
            envelope::ApiResponse<drivers::DriverConfigResponse>,
            envelope::ApiResponse<devices::DeviceListResponse>,
            envelope::ApiResponse<devices::DeviceSummary>,
            envelope::ApiResponse<controls::ControlSurfaceListResponse>,
            envelope::ApiResponse<hypercolor_types::controls::ControlSurfaceDocument>,
            envelope::ApiResponse<hypercolor_types::controls::ApplyControlChangesResponse>,
            envelope::ApiResponse<hypercolor_types::controls::ControlActionResult>,
            envelope::ApiResponse<effects::EffectListResponse>,
            envelope::ApiResponse<effects::EffectDetailResponse>,
            envelope::ApiResponse<effects::ActiveEffectResponse>,
            envelope::ApiResponse<effects::EffectPresetListResponse>,
            envelope::ApiResponse<effects::ApplyEffectResponse>,
            envelope::ApiResponse<hypercolor_types::api::output::OutputResource>,
            envelope::ApiResponse<devices::DeviceBindingsResponse>,
            envelope::ApiResponse<devices::RebindDeviceResponse>,
            devices::UpdateDeviceRequest,
            devices::IdentifyRequest,
            devices::DiscoverRequest,
            devices::RebindDeviceRequest,
            effects::UpdateActiveControlsRequest,
            hypercolor_types::api::output::OutputPatchRequest,
            hypercolor_types::api::output::OutputPowerMode,
            hypercolor_types::api::output::OutputResource,
            layers::BroadcastMediaLayerRequest,
            layers::BroadcastMediaLayerTarget,
            layers::BroadcastMediaLayerZoneResponse,
            layers::BroadcastMediaLayerResponse,
            layers::CreateLayerRequest,
            layers::UpdateLayerRequest,
            layers::LayerOrderRequest,
            layers::PatchLayerControlsRequest,
            layers::LayerStackResponse,
            profiles::ApplyProfileRequest,
            scenes_zones::CreateZoneRequest,
            scenes_zones::UpdateZoneRequest,
            scenes_zones::AssignDevicesRequest,
            scenes_zones::UpdateUnassignedBehaviorRequest,
            scenes_zones::ZoneListResponse,
            scenes_zones::ZoneResponse,
            scenes_zones::ZoneMutationResponse,
            scenes_zones::UnassignedBehaviorResponse,
            config::ConfigMutationResponse,
            hypercolor_types::config_registry::ApplyPolicy,
            hypercolor_types::config_registry::ConfigKeySchemaEntry,
            hypercolor_types::config_registry::LiveSection,
            hypercolor_types::config_registry::Redaction,
            system::SystemStatus,
            system::InputStatus,
            system::InputSourceStatus,
            system::InputSourceIssueStatus,
            system::RenderLoopStatus,
            system::LatestFrameStatus,
            system::RenderSurfaceStatus,
            system::EffectHealthStatus,
            system::PreviewRuntimeStatus,
            system::PreviewDemandStatus,
            system::ServerInfo,
            system::HealthChecks,
            system::HealthResponse,
            hypercolor_types::api::capture::ProtectedSourceGrantOwner,
            hypercolor_types::api::capture::CaptureAuthorizationResponse,
            hypercolor_types::api::capture::CapturePickerResponse,
            hypercolor_types::api::capture::CaptureMonitor,
            drivers::DriverListResponse,
            drivers::DriverSummary,
            drivers::DriverConfigResponse,
            hypercolor_types::config::DriverConfigEntry,
            devices::DeviceListResponse,
            devices::DeviceSummary,
            devices::ZoneSummary,
            devices::ZoneTopologySummary,
            devices::Pagination,
            controls::ControlSurfaceListResponse,
            controls::InvokeControlActionRequest,
            hypercolor_types::controls::ActionConfirmation,
            hypercolor_types::controls::ActionConfirmationLevel,
            hypercolor_types::controls::AppliedControlChange,
            hypercolor_types::controls::ApplyControlChangesRequest,
            hypercolor_types::controls::ApplyControlChangesResponse,
            hypercolor_types::controls::ApplyImpact,
            hypercolor_types::controls::ControlAccess,
            hypercolor_types::controls::ControlActionDescriptor,
            hypercolor_types::controls::ControlActionResult,
            hypercolor_types::controls::ControlActionStatus,
            hypercolor_types::controls::ControlApplyError,
            hypercolor_types::controls::ControlAvailability,
            hypercolor_types::controls::ControlAvailabilityState,
            hypercolor_types::controls::ControlChange,
            hypercolor_types::controls::ControlEnumOption,
            hypercolor_types::controls::ControlFieldDescriptor,
            hypercolor_types::controls::ControlGroupDescriptor,
            hypercolor_types::controls::ControlGroupKind,
            hypercolor_types::controls::ControlObjectField,
            hypercolor_types::controls::ControlOwner,
            hypercolor_types::controls::ControlPersistence,
            hypercolor_types::controls::ControlSurfaceDocument,
            hypercolor_types::controls::ControlSurfaceEvent,
            hypercolor_types::controls::ControlSurfaceScope,
            hypercolor_types::controls::ControlValueKind,
            hypercolor_types::controls::ControlVisibility,
            hypercolor_types::controls::RejectedControlChange,
            effects::ApplyEffectRequest,
            effects::ApplyEffectPresetRequest,
            effects::TransitionRequest,
            effects::EffectListResponse,
            effects::EffectSummary,
            effects::ActiveEffectResponse,
            effects::EffectDetailResponse,
            effects::EffectPresetOrigin,
            effects::EffectPresetSummary,
            effects::EffectPresetListResponse,
            effects::LayoutLinkSummary,
            effects::EffectLayoutApplyResult,
            effects::ApplyTransitionResponse,
            effects::EffectRefSummary,
            effects::ApplyEffectResponse,
            hypercolor_driver_api::DeviceAuthState,
            hypercolor_driver_api::PairingFlowKind,
            hypercolor_driver_api::PairingFieldDescriptor,
            hypercolor_driver_api::PairingDescriptor,
            hypercolor_driver_api::DeviceAuthSummary,
            hypercolor_types::device::DriverModuleKind,
            hypercolor_types::device::DriverTransportKind,
            hypercolor_types::device::DriverCapabilitySet,
            hypercolor_types::device::DeviceClassHint,
            hypercolor_types::device::DriverPresentation,
            hypercolor_types::device::DriverModuleDescriptor,
            hypercolor_types::device::DriverProtocolDescriptor,
            hypercolor_types::server::ServerIdentity,
            hypercolor_types::effect::GradientStop,
            hypercolor_types::effect::ControlType,
            hypercolor_types::effect::ControlKind,
            hypercolor_types::effect::ControlValue,
            hypercolor_types::effect::ControlBinding,
            hypercolor_types::effect::PreviewSource,
            hypercolor_types::effect::ControlDefinition,
            hypercolor_types::effect::PresetTemplate,
            hypercolor_types::spatial::Corner,
            hypercolor_types::spatial::EdgeBehavior,
            hypercolor_types::spatial::LedTopology,
            hypercolor_types::spatial::NormalizedPosition,
            hypercolor_types::spatial::NormalizedRect,
            hypercolor_types::spatial::Orientation,
            hypercolor_types::spatial::Output,
            hypercolor_types::spatial::OutputComponent,
            hypercolor_types::spatial::RingDef,
            hypercolor_types::spatial::RoomAdjacency,
            hypercolor_types::spatial::RoomDimensions,
            hypercolor_types::spatial::SamplingMode,
            hypercolor_types::spatial::SpaceDefinition,
            hypercolor_types::spatial::SpatialLayout,
            hypercolor_types::spatial::StripDirection,
            hypercolor_types::spatial::Wall,
            hypercolor_types::spatial::Winding,
            hypercolor_types::spatial::ZoneShape,
            hypercolor_types::viewport::ViewportRect,
        )
    ),
    tags(
        (name = "system", description = "Daemon identity, health, and status"),
        (name = "drivers", description = "Driver module inventory and capabilities"),
        (name = "devices", description = "Tracked device inventory"),
        (name = "controls", description = "Generic control surfaces and typed value mutation"),
        (name = "effects", description = "Effect catalog and runtime control"),
        (name = "assets", description = "Uploaded media assets"),
        (name = "displays", description = "Display devices, faces, and simulators"),
        (name = "attachments", description = "Physical attachment templates and bindings"),
        (name = "output", description = "Global output power and brightness"),
        (name = "scenes", description = "Scene CRUD and activation"),
        (name = "profiles", description = "Saved lighting profile snapshots"),
        (name = "layouts", description = "Spatial layout CRUD and preview"),
        (name = "library", description = "Favorites, presets, and playlists"),
        (name = "capture", description = "Protected host input and screen-capture actions"),
        (name = "config", description = "Daemon configuration inspection and mutation"),
        (name = "diagnostics", description = "Daemon diagnostics"),
        (name = "websocket", description = "Realtime WebSocket endpoint"),
    ),
    modifiers(&SecurityAddon, &RouteCatalogAddon)
)]
pub(crate) struct ApiDoc;

struct SecurityAddon;
struct RouteCatalogAddon;

#[derive(Clone, Copy)]
pub struct RouteSpec {
    pub method: &'static str,
    pub path: &'static str,
    pub operation_id: &'static str,
    pub tag: &'static str,
    summary: &'static str,
    success_status: &'static str,
    request_body: Option<RequestBodySpec>,
}

#[derive(Clone, Copy)]
struct RequestBodySpec {
    schema: &'static str,
    required: bool,
}

impl RouteSpec {
    const fn get(
        path: &'static str,
        operation_id: &'static str,
        tag: &'static str,
        summary: &'static str,
    ) -> Self {
        Self::new("get", path, operation_id, tag, summary)
    }

    const fn post(
        path: &'static str,
        operation_id: &'static str,
        tag: &'static str,
        summary: &'static str,
    ) -> Self {
        Self::new("post", path, operation_id, tag, summary)
    }

    const fn put(
        path: &'static str,
        operation_id: &'static str,
        tag: &'static str,
        summary: &'static str,
    ) -> Self {
        Self::new("put", path, operation_id, tag, summary)
    }

    const fn patch(
        path: &'static str,
        operation_id: &'static str,
        tag: &'static str,
        summary: &'static str,
    ) -> Self {
        Self::new("patch", path, operation_id, tag, summary)
    }

    const fn delete(
        path: &'static str,
        operation_id: &'static str,
        tag: &'static str,
        summary: &'static str,
    ) -> Self {
        Self::new("delete", path, operation_id, tag, summary)
    }

    const fn new(
        method: &'static str,
        path: &'static str,
        operation_id: &'static str,
        tag: &'static str,
        summary: &'static str,
    ) -> Self {
        Self {
            method,
            path,
            operation_id,
            tag,
            summary,
            success_status: "200",
            request_body: None,
        }
    }

    const fn with_request_body(mut self, schema: &'static str, required: bool) -> Self {
        self.request_body = Some(RequestBodySpec { schema, required });
        self
    }
}

pub const ROUTES: &[RouteSpec] = &[
    RouteSpec::get(
        "/api/v1/assets",
        "list_assets",
        "assets",
        "List media assets",
    ),
    RouteSpec::post(
        "/api/v1/assets",
        "upload_asset",
        "assets",
        "Upload a media asset",
    ),
    RouteSpec::get(
        "/api/v1/assets/{id}",
        "get_asset",
        "assets",
        "Get one media asset",
    ),
    RouteSpec::put(
        "/api/v1/assets/{id}",
        "update_asset",
        "assets",
        "Update one media asset",
    ),
    RouteSpec::delete(
        "/api/v1/assets/{id}",
        "delete_asset",
        "assets",
        "Delete one media asset",
    ),
    RouteSpec::get(
        "/api/v1/assets/{id}/blob",
        "get_asset_blob",
        "assets",
        "Download media asset bytes",
    ),
    RouteSpec::get(
        "/api/v1/assets/{id}/thumbnail",
        "get_asset_thumbnail",
        "assets",
        "Get a media asset thumbnail",
    ),
    RouteSpec::get(
        "/health",
        "health_check",
        "system",
        "Run daemon health check",
    ),
    RouteSpec::get(
        "/api/v1/server",
        "get_server",
        "system",
        "Get daemon server identity",
    ),
    RouteSpec::get(
        "/api/v1/status",
        "get_status",
        "system",
        "Get daemon status",
    ),
    RouteSpec::post(
        "/api/v1/input/authorize",
        "authorize_input_monitoring",
        "capture",
        "Request Input Monitoring authorization",
    ),
    RouteSpec::post(
        "/api/v1/capture/authorize",
        "authorize_screen_recording",
        "capture",
        "Request screen-capture authorization",
    ),
    RouteSpec::post(
        "/api/v1/capture/source/pick",
        "pick_capture_source",
        "capture",
        "Open the screen-capture source picker",
    ),
    RouteSpec::get(
        "/api/v1/capture/monitors",
        "list_capture_monitors",
        "capture",
        "List addressable capture displays",
    ),
    RouteSpec::get(
        "/api/v1/drivers",
        "list_drivers",
        "drivers",
        "List driver modules",
    ),
    RouteSpec::get(
        "/api/v1/drivers/{id}/config",
        "get_driver_config",
        "drivers",
        "Get driver module config",
    ),
    RouteSpec::get(
        "/api/v1/drivers/{id}/controls",
        "get_driver_control_surface",
        "controls",
        "Get driver control surface",
    ),
    RouteSpec::get(
        "/api/v1/system/sensors",
        "get_sensors",
        "system",
        "List system sensors",
    ),
    RouteSpec::get(
        "/api/v1/system/sensors/{label}",
        "get_sensor",
        "system",
        "Get one system sensor",
    ),
    RouteSpec::get(
        "/api/v1/devices",
        "list_devices",
        "devices",
        "List tracked devices",
    ),
    RouteSpec::post(
        "/api/v1/devices/discover",
        "discover_devices",
        "devices",
        "Start device discovery",
    )
    .with_request_body("DiscoverRequest", false),
    RouteSpec::get(
        "/api/v1/devices/metrics",
        "list_device_metrics",
        "devices",
        "List device metrics",
    ),
    RouteSpec::get(
        "/api/v1/devices/bindings",
        "get_device_bindings",
        "devices",
        "List unresolved layout bindings and re-bind candidates",
    ),
    RouteSpec::post(
        "/api/v1/devices/rebind",
        "rebind_device",
        "devices",
        "Re-bind an orphaned layout binding onto an attached claimed device",
    )
    .with_request_body("RebindDeviceRequest", true),
    RouteSpec::get(
        "/api/v1/devices/debug/queues",
        "debug_output_queues",
        "devices",
        "Debug device output queues",
    ),
    RouteSpec::get(
        "/api/v1/devices/debug/routing",
        "debug_device_routing",
        "devices",
        "Debug device output routing",
    ),
    RouteSpec::get(
        "/api/v1/devices/{id}",
        "get_device",
        "devices",
        "Get one device",
    ),
    RouteSpec::put(
        "/api/v1/devices/{id}",
        "update_device",
        "devices",
        "Update one device",
    )
    .with_request_body("UpdateDeviceRequest", true),
    RouteSpec::delete(
        "/api/v1/devices/{id}",
        "delete_device",
        "devices",
        "Delete one device",
    ),
    RouteSpec::get(
        "/api/v1/devices/{id}/controls",
        "get_device_control_surface",
        "controls",
        "Get device control surface",
    ),
    RouteSpec::get(
        "/api/v1/devices/{id}/attachments",
        "get_attachments",
        "devices",
        "Get device attachments",
    ),
    RouteSpec::put(
        "/api/v1/devices/{id}/attachments",
        "update_attachments",
        "devices",
        "Update device attachments",
    ),
    RouteSpec::delete(
        "/api/v1/devices/{id}/attachments",
        "delete_attachments",
        "devices",
        "Delete device attachments",
    ),
    RouteSpec::post(
        "/api/v1/devices/{id}/attachments/preview",
        "preview_attachments",
        "devices",
        "Preview device attachments",
    ),
    RouteSpec::get(
        "/api/v1/devices/{id}/logical-devices",
        "list_device_logical_devices",
        "devices",
        "List logical devices for one physical device",
    ),
    RouteSpec::post(
        "/api/v1/devices/{id}/logical-devices",
        "create_logical_device",
        "devices",
        "Create logical device for one physical device",
    ),
    RouteSpec::post(
        "/api/v1/devices/{id}/identify",
        "identify_device",
        "devices",
        "Identify one device",
    )
    .with_request_body("IdentifyRequest", false),
    RouteSpec::post(
        "/api/v1/devices/{id}/zones/{zone_id}/identify",
        "identify_zone",
        "devices",
        "Identify one device zone",
    ),
    RouteSpec::post(
        "/api/v1/devices/{id}/attachments/{slot_id}/identify",
        "identify_attachment",
        "devices",
        "Identify one attachment",
    ),
    RouteSpec::post(
        "/api/v1/devices/{id}/pair",
        "pair_device",
        "devices",
        "Pair one device",
    ),
    RouteSpec::delete(
        "/api/v1/devices/{id}/pair",
        "delete_pairing",
        "devices",
        "Delete one device pairing",
    ),
    RouteSpec::get(
        "/api/v1/displays",
        "list_displays",
        "displays",
        "List display devices",
    ),
    RouteSpec::get(
        "/api/v1/displays/{id}/preview.jpg",
        "get_display_preview",
        "displays",
        "Get display preview image",
    ),
    RouteSpec::get(
        "/api/v1/displays/{id}/face",
        "get_display_face",
        "displays",
        "Get display face assignment",
    ),
    RouteSpec::put(
        "/api/v1/displays/{id}/face",
        "set_display_face",
        "displays",
        "Set display face assignment",
    ),
    RouteSpec::delete(
        "/api/v1/displays/{id}/face",
        "delete_display_face",
        "displays",
        "Delete display face assignment",
    ),
    RouteSpec::patch(
        "/api/v1/displays/{id}/face/controls",
        "patch_display_face_controls",
        "displays",
        "Patch display face controls",
    ),
    RouteSpec::patch(
        "/api/v1/displays/{id}/face/composition",
        "patch_display_face_composition",
        "displays",
        "Patch display face composition",
    ),
    RouteSpec::get(
        "/api/v1/simulators/displays",
        "list_simulated_displays",
        "displays",
        "List simulated displays",
    ),
    RouteSpec::post(
        "/api/v1/simulators/displays",
        "create_simulated_display",
        "displays",
        "Create simulated display",
    ),
    RouteSpec::get(
        "/api/v1/simulators/displays/{id}",
        "get_simulated_display",
        "displays",
        "Get simulated display",
    ),
    RouteSpec::patch(
        "/api/v1/simulators/displays/{id}",
        "patch_simulated_display",
        "displays",
        "Patch simulated display",
    ),
    RouteSpec::delete(
        "/api/v1/simulators/displays/{id}",
        "delete_simulated_display",
        "displays",
        "Delete simulated display",
    ),
    RouteSpec::get(
        "/api/v1/simulators/displays/{id}/frame",
        "get_simulated_display_frame",
        "displays",
        "Get simulated display frame",
    ),
    RouteSpec::get(
        "/api/v1/logical-devices",
        "list_logical_devices",
        "devices",
        "List logical devices",
    ),
    RouteSpec::get(
        "/api/v1/logical-devices/{id}",
        "get_logical_device",
        "devices",
        "Get logical device",
    ),
    RouteSpec::put(
        "/api/v1/logical-devices/{id}",
        "update_logical_device",
        "devices",
        "Update logical device",
    ),
    RouteSpec::delete(
        "/api/v1/logical-devices/{id}",
        "delete_logical_device",
        "devices",
        "Delete logical device",
    ),
    RouteSpec::get(
        "/api/v1/attachments/templates",
        "list_templates",
        "attachments",
        "List attachment templates",
    ),
    RouteSpec::post(
        "/api/v1/attachments/templates",
        "create_template",
        "attachments",
        "Create attachment template",
    ),
    RouteSpec::get(
        "/api/v1/attachments/templates/{id}",
        "get_template",
        "attachments",
        "Get attachment template",
    ),
    RouteSpec::put(
        "/api/v1/attachments/templates/{id}",
        "update_template",
        "attachments",
        "Update attachment template",
    ),
    RouteSpec::delete(
        "/api/v1/attachments/templates/{id}",
        "delete_template",
        "attachments",
        "Delete attachment template",
    ),
    RouteSpec::get(
        "/api/v1/attachments/categories",
        "list_categories",
        "attachments",
        "List attachment categories",
    ),
    RouteSpec::get(
        "/api/v1/attachments/vendors",
        "list_vendors",
        "attachments",
        "List attachment vendors",
    ),
    RouteSpec::get(
        "/api/v1/output",
        "get_output",
        "output",
        "Get global output power and brightness",
    ),
    RouteSpec::patch(
        "/api/v1/output",
        "patch_output",
        "output",
        "Set global output power, brightness, or both",
    )
    .with_request_body("OutputPatchRequest", true),
    RouteSpec::get("/api/v1/effects", "list_effects", "effects", "List effects"),
    RouteSpec::get(
        "/api/v1/effects/active",
        "get_active_effect",
        "effects",
        "Get active effect",
    ),
    RouteSpec::get(
        "/api/v1/effects/active/cover",
        "get_active_effect_cover",
        "effects",
        "Get active effect cover image",
    ),
    RouteSpec::patch(
        "/api/v1/effects/active/controls",
        "update_active_controls",
        "effects",
        "Update active effect controls",
    )
    .with_request_body("UpdateActiveControlsRequest", true),
    RouteSpec::put(
        "/api/v1/effects/active/controls/{name}/binding",
        "set_active_control_binding",
        "effects",
        "Set active effect control binding",
    ),
    RouteSpec::post(
        "/api/v1/effects/active/reset",
        "reset_controls",
        "effects",
        "Reset active effect controls",
    ),
    RouteSpec::post(
        "/api/v1/effects/stop",
        "stop_effect",
        "effects",
        "Stop active effect",
    ),
    RouteSpec::post(
        "/api/v1/effects/rescan",
        "rescan_effects",
        "effects",
        "Rescan effects",
    ),
    RouteSpec::post(
        "/api/v1/effects/install",
        "install_effect",
        "effects",
        "Install effect",
    ),
    RouteSpec::get(
        "/api/v1/effects/screenshots",
        "get_effect_screenshot",
        "effects",
        "Serve bundled effect screenshots",
    ),
    RouteSpec::get(
        "/api/v1/effects/{id}",
        "get_effect",
        "effects",
        "Get effect",
    ),
    RouteSpec::get(
        "/api/v1/effects/{id}/cover",
        "get_effect_cover",
        "effects",
        "Get effect cover image",
    ),
    RouteSpec::get(
        "/api/v1/effects/{id}/layout",
        "get_effect_layout",
        "effects",
        "Get effect layout link",
    ),
    RouteSpec::put(
        "/api/v1/effects/{id}/layout",
        "set_effect_layout",
        "effects",
        "Set effect layout link",
    ),
    RouteSpec::delete(
        "/api/v1/effects/{id}/layout",
        "delete_effect_layout",
        "effects",
        "Delete effect layout link",
    ),
    RouteSpec::post(
        "/api/v1/effects/{id}/apply",
        "apply_effect",
        "effects",
        "Apply effect",
    ),
    RouteSpec::get(
        "/api/v1/effects/{id}/presets",
        "list_effect_presets",
        "effects",
        "List effect presets",
    ),
    RouteSpec::post(
        "/api/v1/effects/{id}/presets/{preset_id}/apply",
        "apply_effect_preset",
        "effects",
        "Apply effect preset",
    ),
    RouteSpec::patch(
        "/api/v1/effects/{id}/controls",
        "update_effect_controls",
        "effects",
        "Update effect controls",
    ),
    RouteSpec::get("/api/v1/scenes", "list_scenes", "scenes", "List scenes"),
    RouteSpec::post("/api/v1/scenes", "create_scene", "scenes", "Create scene"),
    RouteSpec::get(
        "/api/v1/scenes/active",
        "get_active_scene",
        "scenes",
        "Get active scene",
    ),
    RouteSpec::post(
        "/api/v1/scenes/deactivate",
        "deactivate_scene",
        "scenes",
        "Deactivate active scene",
    ),
    RouteSpec::get("/api/v1/scenes/{id}", "get_scene", "scenes", "Get scene"),
    RouteSpec::put(
        "/api/v1/scenes/{id}",
        "update_scene",
        "scenes",
        "Update scene",
    ),
    RouteSpec::delete(
        "/api/v1/scenes/{id}",
        "delete_scene",
        "scenes",
        "Delete scene",
    ),
    RouteSpec::post(
        "/api/v1/scenes/{id}/activate",
        "activate_scene",
        "scenes",
        "Activate scene",
    ),
    RouteSpec::get(
        "/api/v1/scenes/{id}/zones",
        "list_scene_zones",
        "scenes",
        "List scene zones",
    ),
    RouteSpec::post(
        "/api/v1/scenes/{id}/zones",
        "create_scene_zone",
        "scenes",
        "Create scene zone",
    )
    .with_request_body("CreateZoneRequest", true),
    RouteSpec::get(
        "/api/v1/scenes/{id}/zones/{zone_id}",
        "get_scene_zone",
        "scenes",
        "Get scene zone",
    ),
    RouteSpec::patch(
        "/api/v1/scenes/{id}/zones/{zone_id}",
        "update_scene_zone",
        "scenes",
        "Update scene zone",
    )
    .with_request_body("UpdateZoneRequest", true),
    RouteSpec::delete(
        "/api/v1/scenes/{id}/zones/{zone_id}",
        "delete_scene_zone",
        "scenes",
        "Delete scene zone",
    ),
    RouteSpec::put(
        "/api/v1/scenes/{id}/zones/{zone_id}/layout",
        "update_scene_zone_layout",
        "scenes",
        "Update scene zone layout",
    )
    .with_request_body("SpatialLayout", true),
    RouteSpec::post(
        "/api/v1/scenes/{id}/zones/{zone_id}/devices",
        "assign_scene_zone_devices",
        "scenes",
        "Assign device zones",
    )
    .with_request_body("AssignDevicesRequest", true),
    RouteSpec::delete(
        "/api/v1/scenes/{id}/zones/{zone_id}/devices/{device_zone_id}",
        "unassign_scene_zone_device",
        "scenes",
        "Unassign device zone",
    ),
    RouteSpec::patch(
        "/api/v1/scenes/{id}/unassigned-behavior",
        "update_scene_unassigned_behavior",
        "scenes",
        "Update unassigned behavior",
    )
    .with_request_body("UpdateUnassignedBehaviorRequest", true),
    RouteSpec::post(
        "/api/v1/scenes/{id}/layers/broadcast-media",
        "broadcast_media_layer",
        "scenes",
        "Broadcast one media layer across zones",
    )
    .with_request_body("BroadcastMediaLayerRequest", true),
    RouteSpec::get(
        "/api/v1/scenes/{id}/zones/{zone_id}/layers",
        "list_layers",
        "scenes",
        "List zone layers",
    ),
    RouteSpec::post(
        "/api/v1/scenes/{id}/zones/{zone_id}/layers",
        "create_layer",
        "scenes",
        "Create zone layer",
    )
    .with_request_body("CreateLayerRequest", true),
    RouteSpec::patch(
        "/api/v1/scenes/{id}/zones/{zone_id}/layers/order",
        "reorder_layers",
        "scenes",
        "Reorder zone layers",
    )
    .with_request_body("LayerOrderRequest", true),
    RouteSpec::put(
        "/api/v1/scenes/{id}/zones/{zone_id}/layers/{layer_id}",
        "update_layer",
        "scenes",
        "Update zone layer",
    )
    .with_request_body("UpdateLayerRequest", true),
    RouteSpec::delete(
        "/api/v1/scenes/{id}/zones/{zone_id}/layers/{layer_id}",
        "delete_layer",
        "scenes",
        "Delete zone layer",
    ),
    RouteSpec::patch(
        "/api/v1/scenes/{id}/zones/{zone_id}/layers/{layer_id}/controls",
        "patch_layer_controls",
        "scenes",
        "Patch zone layer controls",
    )
    .with_request_body("PatchLayerControlsRequest", true),
    RouteSpec::get(
        "/api/v1/profiles",
        "list_profiles",
        "profiles",
        "List profiles",
    ),
    RouteSpec::post(
        "/api/v1/profiles",
        "create_profile",
        "profiles",
        "Create profile",
    ),
    RouteSpec::get(
        "/api/v1/profiles/{id}",
        "get_profile",
        "profiles",
        "Get profile",
    ),
    RouteSpec::put(
        "/api/v1/profiles/{id}",
        "update_profile",
        "profiles",
        "Update profile",
    ),
    RouteSpec::delete(
        "/api/v1/profiles/{id}",
        "delete_profile",
        "profiles",
        "Delete profile",
    ),
    RouteSpec::post(
        "/api/v1/profiles/{id}/apply",
        "apply_profile",
        "profiles",
        "Apply profile",
    )
    .with_request_body("ApplyProfileRequest", false),
    RouteSpec::get("/api/v1/layouts", "list_layouts", "layouts", "List layouts"),
    RouteSpec::post(
        "/api/v1/layouts",
        "create_layout",
        "layouts",
        "Create layout",
    ),
    RouteSpec::get(
        "/api/v1/layouts/active",
        "get_active_layout",
        "layouts",
        "Get active layout",
    ),
    RouteSpec::put(
        "/api/v1/layouts/active/preview",
        "preview_layout",
        "layouts",
        "Preview active layout",
    ),
    RouteSpec::get(
        "/api/v1/layouts/{id}",
        "get_layout",
        "layouts",
        "Get layout",
    ),
    RouteSpec::put(
        "/api/v1/layouts/{id}",
        "update_layout",
        "layouts",
        "Update layout",
    ),
    RouteSpec::delete(
        "/api/v1/layouts/{id}",
        "delete_layout",
        "layouts",
        "Delete layout",
    ),
    RouteSpec::post(
        "/api/v1/layouts/{id}/apply",
        "apply_layout",
        "layouts",
        "Apply layout",
    ),
    RouteSpec::get(
        "/api/v1/library/favorites",
        "list_favorites",
        "library",
        "List favorite effects",
    ),
    RouteSpec::post(
        "/api/v1/library/favorites",
        "add_favorite",
        "library",
        "Add favorite effect",
    ),
    RouteSpec::delete(
        "/api/v1/library/favorites/{effect}",
        "remove_favorite",
        "library",
        "Remove favorite effect",
    ),
    RouteSpec::get(
        "/api/v1/library/presets",
        "list_presets",
        "library",
        "List presets",
    ),
    RouteSpec::post(
        "/api/v1/library/presets",
        "create_preset",
        "library",
        "Create preset",
    ),
    RouteSpec::get(
        "/api/v1/library/presets/{id}",
        "get_preset",
        "library",
        "Get preset",
    ),
    RouteSpec::put(
        "/api/v1/library/presets/{id}",
        "update_preset",
        "library",
        "Update preset",
    ),
    RouteSpec::delete(
        "/api/v1/library/presets/{id}",
        "delete_preset",
        "library",
        "Delete preset",
    ),
    RouteSpec::post(
        "/api/v1/library/presets/{id}/apply",
        "apply_preset",
        "library",
        "Apply preset",
    ),
    RouteSpec::get(
        "/api/v1/library/playlists",
        "list_playlists",
        "library",
        "List playlists",
    ),
    RouteSpec::post(
        "/api/v1/library/playlists",
        "create_playlist",
        "library",
        "Create playlist",
    ),
    RouteSpec::get(
        "/api/v1/library/playlists/active",
        "get_active_playlist",
        "library",
        "Get active playlist",
    ),
    RouteSpec::post(
        "/api/v1/library/playlists/stop",
        "stop_playlist",
        "library",
        "Stop playlist",
    ),
    RouteSpec::get(
        "/api/v1/library/playlists/{id}",
        "get_playlist",
        "library",
        "Get playlist",
    ),
    RouteSpec::put(
        "/api/v1/library/playlists/{id}",
        "update_playlist",
        "library",
        "Update playlist",
    ),
    RouteSpec::delete(
        "/api/v1/library/playlists/{id}",
        "delete_playlist",
        "library",
        "Delete playlist",
    ),
    RouteSpec::post(
        "/api/v1/library/playlists/{id}/activate",
        "activate_playlist",
        "library",
        "Activate playlist",
    ),
    RouteSpec::get(
        "/api/v1/system/audio-devices",
        "list_audio_devices",
        "system",
        "List audio input devices",
    ),
    RouteSpec::get(
        "/api/v1/config",
        "show_config",
        "config",
        "Show daemon config",
    ),
    RouteSpec::get(
        "/api/v1/config/schema",
        "get_config_schema",
        "config",
        "Describe every config key",
    ),
    RouteSpec::get(
        "/api/v1/config/keys/{key}",
        "get_config_key",
        "config",
        "Read one daemon config key",
    ),
    RouteSpec::put(
        "/api/v1/config/keys/{key}",
        "put_config_key",
        "config",
        "Write one daemon config key",
    ),
    RouteSpec::delete(
        "/api/v1/config/keys/{key}",
        "delete_config_key",
        "config",
        "Restore one daemon config key to its default",
    ),
    RouteSpec::post(
        "/api/v1/config/reset",
        "reset_config",
        "config",
        "Restore the whole daemon config to defaults",
    ),
    RouteSpec::get(
        "/api/v1/control-surfaces",
        "list_control_surfaces",
        "controls",
        "List control surfaces",
    ),
    RouteSpec::get(
        "/api/v1/control-surfaces/{surface_id}",
        "get_control_surface",
        "controls",
        "Get control surface",
    ),
    RouteSpec::patch(
        "/api/v1/control-surfaces/{surface_id}/values",
        "apply_control_surface_values",
        "controls",
        "Apply control surface values",
    )
    .with_request_body("ApplyControlChangesRequest", true),
    RouteSpec::post(
        "/api/v1/control-surfaces/{surface_id}/actions/{action_id}",
        "invoke_control_surface_action",
        "controls",
        "Invoke control surface action",
    )
    .with_request_body("InvokeControlActionRequest", true),
    RouteSpec::post(
        "/api/v1/diagnose",
        "run_diagnostics",
        "diagnostics",
        "Run daemon diagnostics",
    ),
    RouteSpec::post(
        "/api/v1/diagnose/memory",
        "memory_diagnostics",
        "diagnostics",
        "Run memory diagnostics",
    ),
    RouteSpec::get(
        "/api/v1/ws",
        "ws_handler",
        "websocket",
        "Open realtime WebSocket stream",
    ),
];

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi
            .components
            .get_or_insert_with(utoipa::openapi::Components::new);
        components.add_security_scheme(
            "bearer_auth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("API key")
                    .build(),
            ),
        );
    }
}

impl Modify for RouteCatalogAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        for tag in [
            "assets",
            "displays",
            "controls",
            "attachments",
            "output",
            "scenes",
            "profiles",
            "layouts",
            "library",
            "capture",
            "config",
            "diagnostics",
            "websocket",
        ] {
            ensure_tag(openapi, tag);
        }

        for route in ROUTES {
            let method = http_method(route.method);
            if openapi
                .paths
                .get_path_operation(route.path, method.clone())
                .is_none()
            {
                openapi
                    .paths
                    .add_path_operation(route.path, vec![method], operation(route));
            }
        }
    }
}

pub(crate) fn router() -> SwaggerUi {
    SwaggerUi::new("/api/v1/docs").url("/api/v1/openapi.json", ApiDoc::openapi())
}

pub fn document_json_pretty() -> serde_json::Result<String> {
    serde_json::to_string_pretty(&ApiDoc::openapi())
}

fn operation(route: &RouteSpec) -> utoipa::openapi::path::Operation {
    let mut builder = OperationBuilder::new()
        .tag(route.tag)
        .summary(Some(route.summary))
        .operation_id(Some(route.operation_id))
        .response(
            route.success_status,
            Response::new(format!("{} response", route.summary)),
        )
        .response("400", Response::new("Bad request"))
        .response("404", Response::new("Resource not found"))
        .response("409", Response::new("State conflict"))
        .response("412", Response::new("Precondition failed"))
        .response("422", Response::new("Validation error"))
        .response("500", Response::new("Internal daemon error"));

    for parameter in path_parameters(route.path) {
        builder = builder.parameter(parameter);
    }
    if let Some(request_body) = route.request_body {
        builder = builder.request_body(Some(json_request_body(request_body)));
    }

    builder.build()
}

fn json_request_body(spec: RequestBodySpec) -> utoipa::openapi::request_body::RequestBody {
    RequestBodyBuilder::new()
        .required(Some(if spec.required {
            Required::True
        } else {
            Required::False
        }))
        .content(
            "application/json",
            Content::new(Some(Ref::from_schema_name(spec.schema))),
        )
        .build()
}

fn path_parameters(path: &str) -> Vec<Parameter> {
    let mut parameters = Vec::new();
    let mut remaining = path;

    while let Some(start) = remaining.find('{') {
        let after_start = &remaining[start + 1..];
        let Some(end) = after_start.find('}') else {
            break;
        };
        let name = &after_start[..end];
        parameters.push(
            ParameterBuilder::new()
                .name(name)
                .parameter_in(ParameterIn::Path)
                .required(Required::True)
                .schema(Some(ObjectBuilder::new().schema_type(Type::String)))
                .build(),
        );
        remaining = &after_start[end + 1..];
    }

    parameters
}

fn http_method(method: &str) -> HttpMethod {
    match method {
        "get" => HttpMethod::Get,
        "post" => HttpMethod::Post,
        "put" => HttpMethod::Put,
        "patch" => HttpMethod::Patch,
        "delete" => HttpMethod::Delete,
        _ => unreachable!("route catalog contains only supported HTTP methods"),
    }
}

fn ensure_tag(openapi: &mut utoipa::openapi::OpenApi, name: &str) {
    let tags = openapi.tags.get_or_insert_with(Vec::new);
    if !tags.iter().any(|tag| tag.name == name) {
        tags.push(Tag::new(name));
    }
}
