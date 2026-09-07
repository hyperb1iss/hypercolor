#[cfg(feature = "macos-capture-fixtures")]
use super::super::CpuReductionExecutor;
#[cfg(not(feature = "macos-capture-fixtures"))]
use super::super::ScreenColorTransformCapabilities;
use super::super::{
    Arc, CapturePixelFormat, MacosNativeTargetManifest, MacosPublicationSource,
    MacosScreenRuntimeTelemetry, PlatformGpuApi, RegisteredScreenBranchDemand,
    ResolvedScreenBranchDemand, ResolvedScreenPublicationDescriptor,
    ScreenExecutorColorCapabilities, ScreenPublicationError, ScreenPublicationExecutor,
    ScreenPublicationExecutorFallbackReason, ScreenPublicationExecutorRequest,
};

#[cfg(all(test, feature = "macos-capture-fixtures"))]
pub(in crate::input::screen::macos) fn resolve_macos_publication_branch(
    source: &MacosPublicationSource,
    demand: &RegisteredScreenBranchDemand,
) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
    let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
    resolve_macos_publication_branch_with_telemetry(source, demand, &telemetry)
}

#[cfg(feature = "macos-capture-fixtures")]
pub(in crate::input::screen::macos) fn resolve_macos_publication_branch_with_telemetry(
    source: &MacosPublicationSource,
    demand: &RegisteredScreenBranchDemand,
    telemetry: &Arc<MacosScreenRuntimeTelemetry>,
) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
    let selector = demand.request().selector();
    if !source.matches_selector(selector) {
        return Ok(None);
    }
    let selector = selector.clone();
    let capabilities = CpuReductionExecutor::supported_color_capabilities();
    if matches!(
        demand.request().executor(),
        ScreenPublicationExecutorRequest::Cpu
    ) {
        telemetry.set_cpu();
        return Ok(Some(demand.resolve_with_color_capabilities(
            &source.cpu_source(selector),
            capabilities,
        )?));
    }

    let (target, native_required) = match demand.request().executor() {
        ScreenPublicationExecutorRequest::SourceNative(target) => (target, false),
        ScreenPublicationExecutorRequest::SourceNativeRequired(target) => (target, true),
        ScreenPublicationExecutorRequest::Cpu => {
            unreachable!("CPU publication requests returned above")
        }
    };
    if target.accepted_api() != &PlatformGpuApi::Metal {
        if native_required {
            let reason = ScreenPublicationExecutorFallbackReason::PlatformApiMismatch;
            telemetry.set_native_unavailable(reason, target.id());
            return Err(ScreenPublicationError::RequiredNativeUnavailable(reason).into());
        }
        telemetry.set_cpu_fallback("target_api_not_metal");
    } else if let Ok(native_source) =
        source.gpu_source(selector.clone(), target.physical_gpu_device().clone())
    {
        match demand.resolve_with_executor_capabilities(
            &native_source,
            ScreenExecutorColorCapabilities::new(capabilities, target.color_capabilities()),
        ) {
            Ok(resolved)
                if matches!(
                    resolved.descriptor().executor(),
                    ScreenPublicationExecutor::SourceNative(_)
                ) && MacosNativeTargetManifest::new(resolved.descriptor()).is_ok() =>
            {
                telemetry.set_native(native_required, target.id());
                return Ok(Some(resolved));
            }
            Err(ScreenPublicationError::RequiredNativeUnavailable(reason)) => {
                telemetry.set_native_unavailable(reason, target.id());
                return Err(ScreenPublicationError::RequiredNativeUnavailable(reason).into());
            }
            Ok(_) if native_required => {
                let reason =
                    ScreenPublicationExecutorFallbackReason::NativeColorContractUnsupported;
                telemetry.set_native_unavailable(reason, target.id());
                return Err(ScreenPublicationError::RequiredNativeUnavailable(reason).into());
            }
            Ok(_) => telemetry.set_cpu_fallback("native_contract_unavailable"),
            Err(error) if native_required => return Err(error.into()),
            Err(_) => telemetry.set_cpu_fallback("native_descriptor_incompatible"),
        }
    } else {
        if native_required {
            let reason = ScreenPublicationExecutorFallbackReason::PhysicalGpuDeviceMismatch;
            telemetry.set_native_unavailable(reason, target.id());
            return Err(ScreenPublicationError::RequiredNativeUnavailable(reason).into());
        }
        telemetry.set_cpu_fallback("metal_device_mismatch");
    }

    Ok(Some(demand.resolve_with_color_capabilities(
        &source.cpu_source(selector),
        capabilities,
    )?))
}

#[cfg(not(feature = "macos-capture-fixtures"))]
pub(in crate::input::screen::macos) fn resolve_macos_publication_branch_with_telemetry(
    source: &MacosPublicationSource,
    demand: &RegisteredScreenBranchDemand,
    telemetry: &Arc<MacosScreenRuntimeTelemetry>,
) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
    let selector = demand.request().selector();
    if !source.matches_selector(selector) {
        return Ok(None);
    }
    let (target, native_required) = match demand.request().executor() {
        ScreenPublicationExecutorRequest::SourceNative(target) => (target, false),
        ScreenPublicationExecutorRequest::SourceNativeRequired(target) => (target, true),
        ScreenPublicationExecutorRequest::Cpu => return Ok(None),
    };
    let unavailable = |reason| {
        telemetry.set_native_unavailable(reason, target.id());
        if native_required {
            Err(ScreenPublicationError::RequiredNativeUnavailable(reason).into())
        } else {
            Ok(None)
        }
    };
    if target.accepted_api() != &PlatformGpuApi::Metal {
        return unavailable(ScreenPublicationExecutorFallbackReason::PlatformApiMismatch);
    }
    let Ok(native_source) =
        source.gpu_source(selector.clone(), target.physical_gpu_device().clone())
    else {
        return unavailable(ScreenPublicationExecutorFallbackReason::PhysicalGpuDeviceMismatch);
    };
    let capabilities = ScreenExecutorColorCapabilities::new(
        ScreenColorTransformCapabilities::NONE,
        target.color_capabilities(),
    );
    match demand.resolve_with_executor_capabilities(&native_source, capabilities) {
        Ok(resolved)
            if matches!(
                resolved.descriptor().executor(),
                ScreenPublicationExecutor::SourceNative(_)
            ) && MacosNativeTargetManifest::new(resolved.descriptor()).is_ok() =>
        {
            telemetry.set_native(native_required, target.id());
            Ok(Some(resolved))
        }
        Ok(_) => {
            unavailable(ScreenPublicationExecutorFallbackReason::NativeColorContractUnsupported)
        }
        Err(ScreenPublicationError::RequiredNativeUnavailable(reason)) => unavailable(reason),
        Err(error) if native_required => Err(error.into()),
        Err(_) => {
            unavailable(ScreenPublicationExecutorFallbackReason::NativeColorContractUnsupported)
        }
    }
}

pub(in crate::input::screen::macos) fn macos_native_descriptor_is_identity(
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> bool {
    descriptor.source_pixel_format() == CapturePixelFormat::Bgra8
        && descriptor.source().geometry().crop().is_none()
        && descriptor.geometry().output_extent() == descriptor.source().geometry().storage_extent()
        && descriptor.physical().reduction_extent()
            == descriptor.source().geometry().storage_extent()
        && descriptor.physical().target_pixel_format() == descriptor.source_pixel_format()
        && matches!(
            descriptor.physical().color_pipeline().transform(),
            super::super::super::ResolvedScreenColorTransform::PreserveEncodedSamples
        )
}
