//! Effect-related API types and fetch functions.

use std::collections::HashMap;
use std::future::Future;

use gloo_net::http::Method;
use hypercolor_types::api::scene::{
    ApplyEffectResponse, ClearSceneRequest, ReplaceLayerRequest, SceneDocument, ZoneResource,
};
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::ControlDefinition;
use hypercolor_types::layer::LayerSource;
use hypercolor_types::scene::ZoneRole;
use web_sys::{File, FormData};

use super::{ApiError, ApiResult, client};
use crate::control_surface_api::path_segment;

// ── Types ───────────────────────────────────────────────────────────────────

pub use hypercolor_types::api::effects::{
    EffectCapabilitySet, EffectDetailResponse, EffectListResponse, EffectPresetListResponse,
    EffectPresetOrigin, EffectPresetSummary, EffectSummary, InstalledEffectResponse,
};
pub use hypercolor_types::api::scene::ApplyEffectRequest;

/// UI projection of the top effect layer in the live scene.
#[derive(Debug, Clone, PartialEq)]
pub struct PrimaryEffectView {
    pub id: String,
    pub zone_id: String,
    pub layer_id: String,
    pub name: String,
    pub controls: Vec<ControlDefinition>,
    pub control_values: HashMap<String, ControlValue>,
    pub active_preset_id: Option<String>,
    pub scene_revision: u64,
}

/// Immutable identity of one effect layer observed or created by the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectLayerTarget {
    pub effect_id: String,
    pub zone_id: String,
    pub layer_id: String,
}

impl EffectLayerTarget {
    #[must_use]
    pub fn session_key(&self) -> String {
        format!("{}:{}", self.zone_id, self.layer_id)
    }

    #[must_use]
    pub fn session_ids(key: &str) -> Option<(&str, &str)> {
        key.split_once(':')
    }
}

impl PrimaryEffectView {
    #[must_use]
    pub fn target(&self) -> EffectLayerTarget {
        EffectLayerTarget {
            effect_id: self.id.clone(),
            zone_id: self.zone_id.clone(),
            layer_id: self.layer_id.clone(),
        }
    }
}

// ── Fetch Functions ─────────────────────────────────────────────────────────

/// Fetch all registered effects.
pub async fn fetch_effects() -> ApiResult<Vec<EffectSummary>> {
    let list: EffectListResponse = client::fetch_json("/api/v1/effects").await?;
    Ok(list.items.into_iter().map(route_effect_summary).collect())
}

/// Project the primary zone's top effect layer from the live scene tree.
pub async fn fetch_primary_effect_view() -> ApiResult<Option<PrimaryEffectView>> {
    let scene: SceneDocument = client::fetch_json("/api/v1/scene").await?;
    let Some((zone_id, layer_id, effect_id, values, _, preset_id)) = effect_target(&scene) else {
        return Ok(None);
    };
    let detail = fetch_effect_detail(&effect_id.to_string()).await?;
    Ok(Some(PrimaryEffectView {
        id: effect_id,
        zone_id,
        layer_id,
        name: detail.name,
        controls: detail.controls,
        control_values: values,
        active_preset_id: preset_id,
        scene_revision: scene.revision,
    }))
}

/// Fetch detailed metadata for one effect.
pub async fn fetch_effect_detail(id: &str) -> ApiResult<EffectDetailResponse> {
    let mut detail: EffectDetailResponse =
        client::fetch_json(&format!("/api/v1/effects/{}", path_segment(id))).await?;
    detail.cover_image_url = route_cover_image_url(detail.cover_image_url);
    Ok(detail)
}

fn route_effect_summary(mut effect: EffectSummary) -> EffectSummary {
    effect.cover_image_url = route_cover_image_url(effect.cover_image_url);
    effect
}

fn route_cover_image_url(cover_image_url: Option<String>) -> Option<String> {
    cover_image_url.and_then(|url| client::daemon_url(&url))
}

/// Fetch the bundled and saved preset stack for one effect.
pub async fn fetch_effect_presets(id: &str) -> ApiResult<Vec<EffectPresetSummary>> {
    let response: EffectPresetListResponse =
        client::fetch_json(&format!("/api/v1/effects/{}/presets", path_segment(id))).await?;
    Ok(response.items)
}

/// Apply a bundled or saved preset to an effect and optional render zone.
pub async fn apply_effect_preset(
    effect_id: &str,
    preset_id: &str,
    zone_id: Option<&str>,
    expected_revision: u64,
) -> ApiResult<EffectLayerTarget> {
    let path = format!(
        "/api/v1/effects/{}/presets/{}/apply",
        path_segment(effect_id),
        path_segment(preset_id)
    );
    let zone = zone_id
        .map(|zone_id| {
            uuid::Uuid::parse_str(zone_id)
                .map(hypercolor_types::scene::ZoneId)
                .map_err(|_| ApiError::Serialize("Target zone must be a UUID".to_owned()))
        })
        .transpose()?;
    let body = ApplyEffectRequest {
        zone,
        ..ApplyEffectRequest::default()
    };
    apply_effect_preset_with(effect_id, expected_revision, move |revision| async move {
        client::send_json_versioned::<_, ApplyEffectResponse>(
            Method::POST,
            &path,
            Some(&body),
            Some(revision),
        )
        .await
    })
    .await
}

async fn apply_effect_preset_with<Send, SendFuture>(
    effect_id: &str,
    expected_revision: u64,
    send: Send,
) -> ApiResult<EffectLayerTarget>
where
    Send: FnOnce(u64) -> SendFuture,
    SendFuture: Future<Output = ApiResult<client::MutationOutcome<ApplyEffectResponse>>>,
{
    match send(expected_revision).await? {
        client::MutationOutcome::Applied(response) => {
            effect_target_from_apply(effect_id, &response)
        }
        client::MutationOutcome::Stale { current } => Err(ApiError::Http {
            status: 412,
            message: Some(format!(
                "Scene changed from revision {expected_revision} to {current} before preset apply"
            )),
        }),
    }
}

/// Apply an effect by ID or name. Pass `None` for a bare start; pass
/// `Some(body)` to deliver preferences atomically.
pub async fn apply_effect(
    id: &str,
    body: Option<&ApplyEffectRequest>,
) -> ApiResult<EffectLayerTarget> {
    let path = format!("/api/v1/effects/{}/apply", path_segment(id));
    let body = body.cloned().unwrap_or_default();
    apply_effect_with(id, || async {
        client::post_json::<_, ApplyEffectResponse>(&path, &body).await
    })
    .await
}

async fn apply_effect_with<Send, SendFuture>(
    effect_id: &str,
    send: Send,
) -> ApiResult<EffectLayerTarget>
where
    Send: FnOnce() -> SendFuture,
    SendFuture: Future<Output = ApiResult<ApplyEffectResponse>>,
{
    let response = send().await?;
    effect_target_from_apply(effect_id, &response)
}

/// Stop the currently active effect.
pub async fn stop_effect() -> ApiResult<()> {
    client::post_json_discard("/api/v1/scene/clear", &ClearSceneRequest::default()).await
}

/// Reset one observed effect layer to its defaults.
pub async fn reset_effect_controls(
    target: &EffectLayerTarget,
    expected_revision: u64,
) -> ApiResult<EffectLayerTarget> {
    let scene: SceneDocument = client::fetch_json("/api/v1/scene").await?;
    if scene.revision != expected_revision {
        return Err(ApiError::Http {
            status: 412,
            message: Some(format!(
                "Scene changed from revision {expected_revision} to {} before controls reset",
                scene.revision
            )),
        });
    }
    let zone = scene
        .zones
        .iter()
        .find(|zone| zone.id.to_string() == target.zone_id)
        .ok_or_else(|| {
            ApiError::Parse("The observed effect zone is no longer present".to_owned())
        })?;
    let layer = zone
        .layers
        .iter()
        .find(|layer| layer.id.to_string() == target.layer_id)
        .ok_or_else(|| {
            ApiError::Parse("The observed effect layer is no longer present".to_owned())
        })?;
    let LayerSource::Effect {
        effect_id,
        control_bindings,
        ..
    } = &layer.source
    else {
        return Err(ApiError::Parse(
            "The observed layer no longer runs an effect".to_owned(),
        ));
    };
    if effect_id.to_string() != target.effect_id {
        return Err(ApiError::Parse(
            "The observed layer no longer runs the requested effect".to_owned(),
        ));
    }
    let detail = fetch_effect_detail(&effect_id.to_string()).await?;
    let values: std::collections::HashMap<_, _> = detail
        .controls
        .into_iter()
        .map(|control| (control.control_id().to_owned(), control.default_value))
        .collect();
    let outcome = client::send_json_versioned::<_, ZoneResource>(
        Method::PUT,
        &format!("/api/v1/scene/zones/{}/layers/{}", zone.id, layer.id),
        Some(&ReplaceLayerRequest {
            source: LayerSource::Effect {
                effect_id: *effect_id,
                controls: values,
                control_bindings: control_bindings.clone(),
                preset_id: None,
            },
            name: layer.name.clone(),
            blend: Some(layer.blend),
            opacity: Some(layer.opacity),
            transform: Some(layer.transform),
            adjust: Some(layer.adjust),
            bindings: Some(layer.bindings.clone()),
            enabled: Some(layer.enabled),
        }),
        Some(expected_revision),
    )
    .await?;
    match outcome {
        client::MutationOutcome::Applied(zone) => effect_target_from_zone(&target.effect_id, &zone),
        client::MutationOutcome::Stale { current } => Err(ApiError::Http {
            status: 412,
            message: Some(format!(
                "Scene changed from revision {expected_revision} to {current} before controls reset"
            )),
        }),
    }
}

type EffectTarget = (
    String,
    String,
    String,
    HashMap<String, ControlValue>,
    Vec<String>,
    Option<String>,
);

fn effect_target(scene: &SceneDocument) -> Option<EffectTarget> {
    scene
        .zones
        .iter()
        .find(|zone| zone.role == ZoneRole::Primary)
        .and_then(effect_target_in_zone)
        .or_else(|| scene.zones.iter().find_map(effect_target_in_zone))
}

fn effect_target_in_zone(
    zone: &hypercolor_types::api::scene::ZoneResource,
) -> Option<EffectTarget> {
    zone.layers.iter().rev().find_map(|layer| {
        let LayerSource::Effect {
            effect_id: current_effect_id,
            controls,
            control_bindings,
            preset_id,
        } = &layer.source
        else {
            return None;
        };
        Some((
            zone.id.to_string(),
            layer.id.to_string(),
            current_effect_id.to_string(),
            controls.clone(),
            control_bindings.keys().cloned().collect(),
            preset_id.map(|preset| preset.to_string()),
        ))
    })
}

fn effect_target_from_apply(
    effect_id: &str,
    response: &ApplyEffectResponse,
) -> ApiResult<EffectLayerTarget> {
    effect_target_from_zone(effect_id, &response.zone)
}

fn effect_target_from_zone(effect_id: &str, zone: &ZoneResource) -> ApiResult<EffectLayerTarget> {
    let layer = zone
        .layers
        .iter()
        .rev()
        .find(|layer| {
            matches!(
                &layer.source,
                LayerSource::Effect { effect_id: current, .. }
                    if current.to_string() == effect_id
            )
        })
        .ok_or_else(|| {
            ApiError::Parse("Effect apply response did not contain the created layer".to_owned())
        })?;
    Ok(EffectLayerTarget {
        effect_id: effect_id.to_owned(),
        zone_id: zone.id.to_string(),
        layer_id: layer.id.to_string(),
    })
}

pub async fn upload_effect(file: File) -> ApiResult<InstalledEffectResponse> {
    let form_data = FormData::new().map_err(|error| ApiError::Serialize(format!("{error:?}")))?;
    form_data
        .append_with_blob_and_filename("file", &file, &file.name())
        .map_err(|error| ApiError::Serialize(format!("{error:?}")))?;

    let request = client::request(Method::POST, "/api/v1/effects/install")?;
    let response = request
        .body(form_data)
        .map_err(|error| ApiError::Serialize(error.to_string()))?
        .send()
        .await
        .map_err(|error| ApiError::Network(error.to_string()))?;

    if !(200..300).contains(&response.status()) {
        let status = response.status();
        let payload = response.json::<serde_json::Value>().await.ok();
        let detail_errors = payload
            .as_ref()
            .and_then(|value| value["error"]["details"]["errors"].as_array())
            .map(|errors| {
                errors
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .filter(|joined| !joined.is_empty());
        let message = detail_errors
            .or_else(|| {
                payload
                    .as_ref()
                    .and_then(|value| value["error"]["message"].as_str())
                    .map(str::to_owned)
            })
            .filter(|message| !message.is_empty());
        return Err(ApiError::Http { status, message });
    }

    response
        .json::<hypercolor_types::api::ApiResponse<InstalledEffectResponse>>()
        .await
        .map(|payload| payload.data)
        .map_err(|error| ApiError::Parse(error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::future::Future;
    use std::pin::Pin;
    use std::rc::Rc;
    use std::task::{Context, Poll, Waker};

    use hypercolor_types::api::scene::{
        ApplyEffectResponse, SceneDocument, SideEffectOutcome, TransitionType,
    };

    use super::{apply_effect_preset_with, apply_effect_with};
    use crate::api::{ApiResult, MutationOutcome};

    struct SuspendedApply {
        response: Option<ApplyEffectResponse>,
        observed_layer: Rc<RefCell<String>>,
        next_layer: String,
        suspended: bool,
    }

    impl Future for SuspendedApply {
        type Output = ApiResult<ApplyEffectResponse>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if !self.suspended {
                self.suspended = true;
                self.observed_layer.replace(self.next_layer.clone());
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            Poll::Ready(Ok(self.response.take().expect("response is returned once")))
        }
    }

    struct SuspendedPresetApply {
        apply: SuspendedApply,
    }

    impl Future for SuspendedPresetApply {
        type Output = ApiResult<MutationOutcome<ApplyEffectResponse>>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            match Pin::new(&mut self.get_mut().apply).poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Ok(response)) => Poll::Ready(Ok(MutationOutcome::Applied(response))),
                Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            }
        }
    }

    fn apply_response(effect_id: &str, layer_ids: &[&str]) -> ApplyEffectResponse {
        let scene: SceneDocument = serde_json::from_value(serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "name": "live",
            "kind": "ephemeral",
            "is_default": true,
            "revision": 2,
            "zones": [{
                "id": "00000000-0000-0000-0000-000000000002",
                "name": "primary",
                "role": "primary",
                "enabled": true,
                "brightness": 1.0,
                "members": [],
                "layers": layer_ids.iter().map(|layer_id| serde_json::json!({
                    "id": layer_id,
                    "source": {"type": "effect", "effect_id": effect_id, "controls": {}}
                })).collect::<Vec<_>>()
            }]
        }))
        .expect("apply response fixture should deserialize");
        ApplyEffectResponse {
            zone: scene.zones[0].clone(),
            transition: TransitionType::Cut,
            output: SideEffectOutcome::applied(),
        }
    }

    fn poll_twice<T>(future: impl Future<Output = T>) -> T {
        let mut future = std::pin::pin!(future);
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
        let Poll::Ready(output) = future.as_mut().poll(&mut context) else {
            panic!("suspended request should resolve on its second poll");
        };
        output
    }

    #[test]
    fn direct_same_effect_reapply_returns_the_new_layer_identity() {
        let effect = "00000000-0000-0000-0000-00000000000a";
        let old_layer = "00000000-0000-0000-0000-000000000003";
        let new_layer = "00000000-0000-0000-0000-000000000004";
        let observed_layer = Rc::new(RefCell::new(old_layer.to_owned()));
        let request_observed_layer = Rc::clone(&observed_layer);
        let result = poll_twice(apply_effect_with(effect, move || SuspendedApply {
            response: Some(apply_response(effect, &[old_layer, new_layer])),
            observed_layer: request_observed_layer,
            next_layer: new_layer.to_owned(),
            suspended: false,
        }))
        .expect("same-effect reapply should return its layer target");

        assert_eq!(observed_layer.borrow().as_str(), new_layer);
        assert_eq!(result.effect_id, effect);
        assert_eq!(result.layer_id, new_layer);
    }

    #[test]
    fn preset_same_effect_apply_returns_the_replacement_layer_identity() {
        let effect = "00000000-0000-0000-0000-00000000000a";
        let old_layer = "00000000-0000-0000-0000-000000000003";
        let new_layer = "00000000-0000-0000-0000-000000000004";
        let observed_layer = Rc::new(RefCell::new(old_layer.to_owned()));
        let request_observed_layer = Rc::clone(&observed_layer);
        let result = poll_twice(apply_effect_preset_with(effect, 7, move |_| {
            SuspendedPresetApply {
                apply: SuspendedApply {
                    response: Some(apply_response(effect, &[old_layer, new_layer])),
                    observed_layer: request_observed_layer,
                    next_layer: new_layer.to_owned(),
                    suspended: false,
                },
            }
        }))
        .expect("preset apply should return its replacement target");

        assert_eq!(observed_layer.borrow().as_str(), new_layer);
        assert_eq!(result.effect_id, effect);
        assert_eq!(result.layer_id, new_layer);
    }

    #[test]
    fn effect_cover_urls_require_verified_native_route_and_preserve_browser_same_origin() {
        crate::api::client::reset_daemon_transport_for_test();
        let route = Some("/api/v1/effects/prism/cover".to_owned());
        assert_eq!(super::route_cover_image_url(route.clone()), route);

        crate::api::client::begin_native_daemon_verification();
        assert_eq!(super::route_cover_image_url(route.clone()), None);

        crate::api::client::install_verified_daemon_connection(
            "http://127.0.0.1:9420",
            Some("protected"),
        );
        assert_eq!(
            super::route_cover_image_url(route),
            Some("http://127.0.0.1:9420/api/v1/effects/prism/cover".to_owned())
        );
        crate::api::client::reset_daemon_transport_for_test();
    }
}
