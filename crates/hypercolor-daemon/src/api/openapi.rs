#![allow(clippy::needless_for_each)]

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_swagger_ui::SwaggerUi;

use crate::api::{devices, effects, envelope, system};

#[derive(OpenApi)]
#[openapi(
    paths(
        system::health_check,
        system::get_server,
        system::get_status,
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
            envelope::ApiResponse<devices::DeviceListResponse>,
            envelope::ApiResponse<devices::DeviceSummary>,
            envelope::ApiResponse<effects::EffectListResponse>,
            envelope::ApiResponse<effects::EffectDetailResponse>,
            envelope::ApiResponse<effects::ActiveEffectResponse>,
            envelope::ApiResponse<effects::ApplyEffectResponse>,
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
            devices::DeviceListResponse,
            devices::DeviceSummary,
            devices::ZoneSummary,
            devices::ZoneTopologySummary,
            devices::Pagination,
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
        (name = "devices", description = "Tracked device inventory"),
        (name = "effects", description = "Effect catalog and runtime control"),
    ),
    modifiers(&SecurityAddon)
)]
pub(crate) struct ApiDoc;

struct SecurityAddon;

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

pub(crate) fn router() -> SwaggerUi {
    SwaggerUi::new("/api/v1/docs").url("/api/v1/openapi.json", ApiDoc::openapi())
}
