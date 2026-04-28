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
    config, controls, devices, drivers, effects, envelope, profiles, settings, system,
};

#[derive(OpenApi)]
#[openapi(
    paths(
        system::health_check,
        system::get_server,
        system::get_status,
        drivers::list_drivers,
        devices::list_devices,
        devices::get_device,
        effects::list_effects,
        effects::get_effect,
        effects::get_active_effect,
        effects::apply_effect,
    ),
    components(
        schemas(
            envelope::Meta,
            envelope::ErrorCode,
            envelope::ErrorBody,
            envelope::ApiErrorResponse,
            envelope::ApiResponse<system::SystemStatus>,
            envelope::ApiResponse<system::ServerInfo>,
            envelope::ApiResponse<drivers::DriverListResponse>,
            envelope::ApiResponse<devices::DeviceListResponse>,
            envelope::ApiResponse<devices::DeviceSummary>,
            envelope::ApiResponse<controls::ControlSurfaceListResponse>,
            envelope::ApiResponse<hypercolor_types::controls::ControlSurfaceDocument>,
            envelope::ApiResponse<hypercolor_types::controls::ApplyControlChangesResponse>,
            envelope::ApiResponse<hypercolor_types::controls::ControlActionResult>,
            envelope::ApiResponse<effects::EffectListResponse>,
            envelope::ApiResponse<effects::EffectDetailResponse>,
            envelope::ApiResponse<effects::ActiveEffectResponse>,
            envelope::ApiResponse<effects::ApplyEffectResponse>,
            devices::UpdateDeviceRequest,
            devices::IdentifyRequest,
            devices::DiscoverRequest,
            effects::UpdateCurrentControlsRequest,
            profiles::ApplyProfileRequest,
            settings::SetBrightnessRequest,
            config::SetConfigRequest,
            system::SystemStatus,
            system::RenderLoopStatus,
            system::LatestFrameStatus,
            system::RenderSurfaceStatus,
            system::EffectHealthStatus,
            system::PreviewRuntimeStatus,
            system::PreviewDemandStatus,
            system::ServerInfo,
            system::HealthChecks,
            system::HealthResponse,
            drivers::DriverListResponse,
            drivers::DriverSummary,
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
            effects::TransitionRequest,
            effects::EffectListResponse,
            effects::EffectSummary,
            effects::ActiveEffectResponse,
            effects::EffectDetailResponse,
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
            hypercolor_types::viewport::ViewportRect,
        )
    ),
    tags(
        (name = "system", description = "Daemon identity, health, and status"),
        (name = "drivers", description = "Driver module inventory and capabilities"),
        (name = "devices", description = "Tracked device inventory"),
        (name = "controls", description = "Generic control surfaces and typed value mutation"),
        (name = "effects", description = "Effect catalog and runtime control"),
        (name = "displays", description = "Display devices, faces, and simulators"),
        (name = "attachments", description = "Physical attachment templates and bindings"),
        (name = "scenes", description = "Scene CRUD and activation"),
        (name = "profiles", description = "Saved lighting profile snapshots"),
        (name = "layouts", description = "Spatial layout CRUD and preview"),
        (name = "library", description = "Favorites, presets, and playlists"),
        (name = "settings", description = "Runtime settings and audio inputs"),
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
    RouteSpec::get(
        "/api/v1/drivers",
        "list_drivers",
        "drivers",
        "List driver modules",
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
    RouteSpec::get("/api/v1/effects", "list_effects", "effects", "List effects"),
    RouteSpec::get(
        "/api/v1/effects/active",
        "get_active_effect",
        "effects",
        "Get active effect",
    ),
    RouteSpec::patch(
        "/api/v1/effects/current/controls",
        "update_current_controls",
        "effects",
        "Update current effect controls",
    )
    .with_request_body("UpdateCurrentControlsRequest", true),
    RouteSpec::put(
        "/api/v1/effects/current/controls/{name}/binding",
        "set_current_control_binding",
        "effects",
        "Set current effect control binding",
    ),
    RouteSpec::post(
        "/api/v1/effects/current/reset",
        "reset_controls",
        "effects",
        "Reset current effect controls",
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
        "/api/v1/effects/{id}",
        "get_effect",
        "effects",
        "Get effect",
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
        "/api/v1/audio/devices",
        "list_audio_devices",
        "settings",
        "List audio input devices",
    ),
    RouteSpec::get(
        "/api/v1/settings/brightness",
        "get_brightness",
        "settings",
        "Get global brightness",
    ),
    RouteSpec::put(
        "/api/v1/settings/brightness",
        "set_brightness",
        "settings",
        "Set global brightness",
    )
    .with_request_body("SetBrightnessRequest", true),
    RouteSpec::get(
        "/api/v1/config",
        "show_config",
        "config",
        "Show daemon config",
    ),
    RouteSpec::get(
        "/api/v1/config/get",
        "get_config_value",
        "config",
        "Get daemon config value",
    ),
    RouteSpec::post(
        "/api/v1/config/set",
        "set_config_value",
        "config",
        "Set daemon config value",
    )
    .with_request_body("SetConfigRequest", true),
    RouteSpec::post(
        "/api/v1/config/reset",
        "reset_config_value",
        "config",
        "Reset daemon config value",
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
            "displays",
            "controls",
            "attachments",
            "scenes",
            "profiles",
            "layouts",
            "library",
            "settings",
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
