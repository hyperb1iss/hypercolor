//! Integration tests for CLI request payloads sent to the daemon API.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axum::extract::{Path, Query, State};
use axum::http::Uri;
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use hypercolor_types::api::system::{ServerInfo, SystemResource, SystemStatus};
use tokio::sync::{Mutex, oneshot};

type SharedBody = Arc<Mutex<Option<serde_json::Value>>>;
type SharedUri = Arc<Mutex<Option<String>>>;
type SharedRequest = (SharedUri, SharedBody);

async fn run_hyper_output(port: u16, args: &[&str]) -> Result<std::process::Output> {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_hypercolor"));
    cmd.arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg("--json")
        .args(args);

    let output = tokio::time::timeout(Duration::from_secs(10), cmd.output())
        .await
        .context("timed out waiting for hyper CLI process")?
        .context("failed to execute hyper CLI")?;
    Ok(output)
}

/// Run the CLI in plain text mode, where --json would otherwise win.
async fn run_hyper_plain(port: u16, args: &[&str]) -> Result<String> {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_hypercolor"));
    cmd.arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg("--format")
        .arg("plain")
        .args(args);

    let output = tokio::time::timeout(Duration::from_secs(10), cmd.output())
        .await
        .context("timed out waiting for hyper CLI process")?
        .context("failed to execute hyper CLI")?;
    if !output.status.success() {
        bail!(
            "hyper CLI failed (status={}):\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn run_hyper(port: u16, args: &[&str]) -> Result<()> {
    let output = run_hyper_output(port, args).await?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "hyper CLI failed (status={}):\nstdout:\n{}\nstderr:\n{}",
            output.status,
            stdout,
            stderr
        );
    }

    Ok(())
}

async fn spawn_server(
    router: Router,
) -> Result<(u16, oneshot::Sender<()>, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("failed to bind test listener")?;
    let port = listener
        .local_addr()
        .context("failed to inspect test listener address")?
        .port();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;
    });
    Ok((port, shutdown_tx, task))
}

#[tokio::test]
async fn effects_activate_serializes_scalar_params() -> Result<()> {
    let captured_body: SharedBody = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route("/api/v1/effects/{effect}", get(effect_detail_with_color))
        .route("/api/v1/effects/{effect}/apply", post(capture_effect_apply))
        .with_state(Arc::clone(&captured_body));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let cli_result = run_hyper(
        port,
        &[
            "effects",
            "activate",
            "demo",
            "--param",
            "speed=12.5",
            "--param",
            "enabled=true",
            "--param",
            "label=aurora",
        ],
    )
    .await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    cli_result?;

    let body = captured_body
        .lock()
        .await
        .clone()
        .context("server did not capture effect apply request body")?;
    assert_eq!(
        body["controls"]["speed"],
        serde_json::json!({ "kind": "float", "value": 12.5 })
    );
    assert_eq!(
        body["controls"]["enabled"],
        serde_json::json!({ "kind": "bool", "value": true })
    );
    assert_eq!(
        body["controls"]["label"],
        serde_json::json!({ "kind": "text", "value": "aurora" })
    );
    assert!(body.get("transition").is_none());

    Ok(())
}

#[tokio::test]
async fn effects_activate_uses_color_schema_for_four_channel_params() -> Result<()> {
    let captured_body: SharedBody = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route("/api/v1/effects/{effect}", get(effect_detail_with_color))
        .route("/api/v1/effects/{effect}/apply", post(capture_effect_apply))
        .with_state(Arc::clone(&captured_body));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let cli_result = run_hyper(
        port,
        &[
            "effects",
            "activate",
            "demo",
            "--param",
            "accent=[0.125,0.25,0.5,1.0]",
        ],
    )
    .await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    cli_result?;

    let body = captured_body
        .lock()
        .await
        .clone()
        .context("server did not capture effect apply request body")?;
    assert_eq!(
        body["controls"]["accent"],
        serde_json::json!({
            "kind": "color_linear",
            "value": {"r": 0.125, "g": 0.25, "b": 0.5, "a": 1.0}
        })
    );

    Ok(())
}

#[tokio::test]
async fn effects_activate_rejects_ambiguous_arrays_without_color_schema() -> Result<()> {
    let captured_body: SharedBody = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route("/api/v1/effects/{effect}", get(effect_detail_with_color))
        .route("/api/v1/effects/{effect}/apply", post(capture_effect_apply))
        .with_state(Arc::clone(&captured_body));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let output = run_hyper_output(
        port,
        &[
            "effects",
            "activate",
            "demo",
            "--param",
            "unknown=[0.125,0.25,0.5,1.0]",
        ],
    )
    .await?;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;

    assert!(!output.status.success());
    assert!(captured_body.lock().await.is_none());

    Ok(())
}

#[tokio::test]
async fn effects_patch_uses_active_effect_color_schema() -> Result<()> {
    const SCENE_ID: &str = "0198c5b6-2222-7000-8000-000000000001";
    const ZONE_ID: &str = "0198c5b6-2222-7000-8000-000000000002";
    const LAYER_ID: &str = "0198c5b6-2222-7000-8000-000000000003";
    const EFFECT_ID: &str = "0198c5b6-2222-7000-8000-000000000004";

    let captured_body: SharedBody = Arc::new(Mutex::new(None));
    let router =
        Router::new()
            .route(
                "/api/v1/scene",
                get(|| async {
                    Json(serde_json::json!({
                        "data": {
                            "id": SCENE_ID,
                            "name": "Desk",
                            "kind": "named",
                            "is_default": false,
                            "unassigned_behavior": "off",
                            "layout_id": null,
                            "revision": 4,
                            "zones": [{
                                "id": ZONE_ID,
                                "name": "Primary",
                                "role": "primary",
                                "enabled": true,
                                "brightness": 1.0,
                                "members": [],
                                "layout": null,
                                "layers": [{
                                    "id": LAYER_ID,
                                    "source": {
                                        "type": "effect",
                                        "effect_id": EFFECT_ID,
                                        "controls": {}
                                    },
                                    "blend": "replace",
                                    "opacity": 1.0
                                }]
                            }]
                        }
                    }))
                }),
            )
            .route("/api/v1/effects/{effect}", get(effect_detail_with_color))
            .route(
                "/api/v1/scene/zones/{zone}/layers/{layer}/controls",
                patch(
                    |State(captured_body): State<SharedBody>,
                     Json(body): Json<serde_json::Value>| async move {
                        *captured_body.lock().await = Some(body);
                        Json(serde_json::json!({
                            "data": zone_resource_fixture(ZONE_ID, LAYER_ID, EFFECT_ID)
                        }))
                    },
                ),
            )
            .with_state(Arc::clone(&captured_body));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let cli_result = run_hyper(
        port,
        &["effects", "patch", "--param", "accent=[0.125,0.25,0.5,1.0]"],
    )
    .await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    cli_result?;

    let body = captured_body
        .lock()
        .await
        .clone()
        .context("server did not capture effect control patch")?;
    assert_eq!(body["values"]["accent"]["kind"], "color_linear");
    assert_eq!(body["values"]["accent"]["value"]["b"], 0.5);

    Ok(())
}

#[tokio::test]
async fn effects_reset_replaces_the_real_layer_without_reapplying() -> Result<()> {
    const SCENE_ID: &str = "0198c5b6-1111-7000-8000-000000000001";
    const ZONE_ID: &str = "0198c5b6-1111-7000-8000-000000000002";
    const LAYER_ID: &str = "0198c5b6-1111-7000-8000-000000000003";
    const EFFECT_ID: &str = "0198c5b6-1111-7000-8000-000000000004";

    let captured_uri: SharedUri = Arc::new(Mutex::new(None));
    let captured_body: SharedBody = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route(
            "/api/v1/scene",
            get(|| async {
                Json(serde_json::json!({
                    "data": {
                        "id": SCENE_ID,
                        "name": "Desk",
                        "kind": "named",
                        "is_default": false,
                        "unassigned_behavior": "off",
                        "layout_id": null,
                        "revision": 4,
                        "zones": [{
                            "id": ZONE_ID,
                            "name": "Primary",
                            "role": "primary",
                            "enabled": true,
                            "brightness": 1.0,
                            "members": [],
                            "layout": null,
                            "layers": [{
                                "id": LAYER_ID,
                                "source": {
                                    "type": "effect",
                                    "effect_id": EFFECT_ID,
                                    "controls": {
                                        "speed": { "kind": "float", "value": 0.9 }
                                    }
                                },
                                "blend": "replace",
                                "opacity": 1.0
                            }]
                        }]
                    }
                }))
            }),
        )
        .route(
            "/api/v1/effects/{id}",
            get(|Path(id): Path<String>| async move {
                assert_eq!(id, EFFECT_ID);
                Json(serde_json::json!({
                    "data": {
                        "id": EFFECT_ID,
                        "name": "Rainbow",
                        "description": "test",
                        "author": "test",
                        "category": "ambient",
                        "source": "native",
                        "runnable": true,
                        "tags": [],
                        "version": "1",
                        "audio_reactive": false,
                        "controls": []
                    }
                }))
            }),
        )
        .route(
            "/api/v1/scene/zones/{zone}/layers/{layer}",
            put(
                |State((captured_uri, captured_body)): State<SharedRequest>,
                 uri: Uri,
                 Json(body): Json<serde_json::Value>| async move {
                    *captured_uri.lock().await = Some(uri.to_string());
                    *captured_body.lock().await = Some(body);
                    Json(serde_json::json!({
                        "data": zone_resource_fixture(ZONE_ID, LAYER_ID, EFFECT_ID)
                    }))
                },
            ),
        )
        .with_state((Arc::clone(&captured_uri), Arc::clone(&captured_body)));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let cli_result = run_hyper(port, &["effects", "reset"]).await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    cli_result?;

    assert_eq!(
        captured_uri.lock().await.as_deref(),
        Some(
            "/api/v1/scene/zones/0198c5b6-1111-7000-8000-000000000002/layers/0198c5b6-1111-7000-8000-000000000003"
        )
    );
    let body = captured_body
        .lock()
        .await
        .clone()
        .context("server did not capture layer replacement")?;
    assert_eq!(body["source"]["effect_id"], EFFECT_ID);
    assert_eq!(body["source"]["controls"], serde_json::json!({}));
    assert!(body["source"].get("preset_id").is_none());

    Ok(())
}

#[tokio::test]
async fn controls_show_full_driver_device_surface_fetches_surface_by_id() -> Result<()> {
    let captured_uri: SharedUri = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route(
            "/api/v1/control-surfaces/{id}",
            get(
                |Path(surface_id): Path<String>,
                 State(captured_uri): State<SharedUri>,
                 uri: Uri| async move {
                    assert_eq!(surface_id, "driver:wled:device:Desk Strip");
                    *captured_uri.lock().await = Some(uri.to_string());
                    Json(serde_json::json!({
                        "data": {
                            "surface_id": "driver:wled:device:Desk Strip",
                            "scope": {
                                "device": {
                                    "device_id": "00000000-0000-0000-0000-000000000001",
                                    "driver_id": "wled"
                                }
                            },
                            "schema_version": 1,
                            "revision": 7,
                            "groups": [],
                            "fields": [],
                            "actions": [],
                            "values": {},
                            "availability": {},
                            "action_availability": {}
                        }
                    }))
                },
            ),
        )
        .with_state(Arc::clone(&captured_uri));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let cli_result = run_hyper(port, &["controls", "show", "driver:wled:device:Desk Strip"]).await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    cli_result?;

    assert_eq!(
        captured_uri.lock().await.as_deref(),
        Some("/api/v1/control-surfaces/driver%3Awled%3Adevice%3ADesk%20Strip")
    );

    Ok(())
}

#[tokio::test]
async fn drivers_set_control_targets_driver_surface() -> Result<()> {
    let captured_uri: SharedUri = Arc::new(Mutex::new(None));
    let captured_body: SharedBody = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route(
            "/api/v1/drivers/{driver}/controls",
            get(driver_control_surface),
        )
        .route(
            "/api/v1/control-surfaces/{id}/values",
            patch(capture_control_patch),
        )
        .with_state((Arc::clone(&captured_uri), Arc::clone(&captured_body)));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let cli_result = run_hyper(
        port,
        &[
            "drivers",
            "set-control",
            "wled",
            "default_protocol",
            "enum:ddp",
        ],
    )
    .await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    cli_result?;

    assert_eq!(
        captured_uri.lock().await.as_deref(),
        Some("/api/v1/control-surfaces/driver%3Awled/values")
    );
    assert_eq!(
        captured_body
            .lock()
            .await
            .clone()
            .context("server did not capture control patch request body")?,
        serde_json::json!({
            "values": {
                "default_protocol": {
                    "kind": "enum",
                    "value": "ddp"
                }
            }
        })
    );

    Ok(())
}

#[tokio::test]
async fn drivers_controls_fetches_driver_surface_endpoint() -> Result<()> {
    let captured_uri: SharedUri = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route(
            "/api/v1/drivers/{driver}/controls",
            get(capture_driver_control_surface),
        )
        .with_state(Arc::clone(&captured_uri));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let cli_result = run_hyper(port, &["drivers", "controls", "wled"]).await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    cli_result?;

    assert_eq!(
        captured_uri.lock().await.as_deref(),
        Some("/api/v1/drivers/wled/controls")
    );

    Ok(())
}

#[tokio::test]
async fn drivers_action_targets_driver_surface() -> Result<()> {
    let captured_uri: SharedUri = Arc::new(Mutex::new(None));
    let captured_body: SharedBody = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route(
            "/api/v1/drivers/{driver}/controls",
            get(driver_control_surface),
        )
        .route(
            "/api/v1/control-surfaces/{id}/actions/{action}",
            post(capture_control_action),
        )
        .with_state((Arc::clone(&captured_uri), Arc::clone(&captured_body)));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let cli_result = run_hyper(
        port,
        &[
            "drivers",
            "action",
            "wled",
            "rescan",
            "--input",
            "force=bool:true",
        ],
    )
    .await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    cli_result?;

    assert_eq!(
        captured_uri.lock().await.as_deref(),
        Some("/api/v1/control-surfaces/driver%3Awled/actions/rescan")
    );
    assert_eq!(
        captured_body
            .lock()
            .await
            .clone()
            .context("server did not capture control action request body")?,
        serde_json::json!({
            "input": {
                "force": {
                    "kind": "bool",
                    "value": true
                }
            }
        })
    );

    Ok(())
}

#[tokio::test]
async fn drivers_action_requires_confirmation_without_yes() -> Result<()> {
    let captured_uri: SharedUri = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route(
            "/api/v1/drivers/{driver}/controls",
            get(confirmed_driver_control_surface),
        )
        .with_state(Arc::clone(&captured_uri));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let output = run_hyper_output(port, &["drivers", "action", "wled", "factory_reset"]).await?;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;

    assert!(!output.status.success());
    assert_eq!(
        captured_uri.lock().await.as_deref(),
        Some("/api/v1/drivers/wled/controls")
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Use --yes to confirm action 'factory_reset'"),
        "stderr should explain confirmation failure: {stderr}"
    );

    Ok(())
}

#[tokio::test]
async fn devices_set_control_targets_device_surface() -> Result<()> {
    let captured_uri: SharedUri = Arc::new(Mutex::new(None));
    let captured_body: SharedBody = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route(
            "/api/v1/control-surfaces",
            get(capture_device_control_surface_list),
        )
        .route(
            "/api/v1/control-surfaces/{id}/values",
            patch(capture_device_control_patch),
        )
        .with_state((Arc::clone(&captured_uri), Arc::clone(&captured_body)));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let cli_result = run_hyper(
        port,
        &[
            "devices",
            "set-control",
            test_device_id(),
            "color_order",
            "enum:grb",
        ],
    )
    .await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    cli_result?;

    assert_eq!(
        captured_uri.lock().await.as_deref(),
        Some(
            "/api/v1/control-surfaces/driver%3Awled%3Adevice%3A00000000-0000-0000-0000-000000000001/values"
        )
    );
    assert_eq!(
        captured_body
            .lock()
            .await
            .clone()
            .context("server did not capture device control patch request body")?,
        serde_json::json!({
            "values": {
                "color_order": {
                    "kind": "enum",
                    "value": "grb"
                }
            }
        })
    );

    Ok(())
}

#[tokio::test]
async fn devices_controls_fetches_device_surface_list_endpoint() -> Result<()> {
    let captured_uri: SharedUri = Arc::new(Mutex::new(None));
    let captured_body: SharedBody = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route(
            "/api/v1/control-surfaces",
            get(capture_device_control_surface_list),
        )
        .with_state((Arc::clone(&captured_uri), Arc::clone(&captured_body)));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let cli_result = run_hyper(port, &["devices", "controls", test_device_id()]).await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    cli_result?;

    assert_eq!(
        captured_uri.lock().await.as_deref(),
        Some("/api/v1/control-surfaces?device_id=00000000-0000-0000-0000-000000000001")
    );

    Ok(())
}

#[tokio::test]
async fn devices_action_targets_device_surface() -> Result<()> {
    let captured_uri: SharedUri = Arc::new(Mutex::new(None));
    let captured_body: SharedBody = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route(
            "/api/v1/control-surfaces",
            get(capture_device_control_surface_list),
        )
        .route(
            "/api/v1/control-surfaces/{id}/actions/{action}",
            post(capture_device_control_action),
        )
        .with_state((Arc::clone(&captured_uri), Arc::clone(&captured_body)));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let cli_result = run_hyper(
        port,
        &[
            "devices",
            "action",
            test_device_id(),
            "identify",
            "--input",
            "duration_ms=duration:1200",
        ],
    )
    .await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    cli_result?;

    assert_eq!(
        captured_uri.lock().await.as_deref(),
        Some(
            "/api/v1/control-surfaces/device%3A00000000-0000-0000-0000-000000000001/actions/identify"
        )
    );
    assert_eq!(
        captured_body
            .lock()
            .await
            .clone()
            .context("server did not capture device control action request body")?,
        serde_json::json!({
            "input": {
                "duration_ms": {
                    "kind": "duration",
                    "value": 1200
                }
            }
        })
    );

    Ok(())
}

#[tokio::test]
async fn scenes_snapshot_sends_name_and_description() -> Result<()> {
    let captured_body: SharedBody = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route("/api/v1/scenes/snapshot", post(capture_scene_snapshot))
        .with_state(Arc::clone(&captured_body));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let cli_result = run_hyper(
        port,
        &[
            "scenes",
            "snapshot",
            "evening",
            "--description",
            "Warm evening light",
        ],
    )
    .await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    cli_result?;

    let body = captured_body
        .lock()
        .await
        .clone()
        .context("server did not capture scene snapshot request body")?;
    assert_eq!(
        body,
        serde_json::json!({
            "name": "evening",
            "description": "Warm evening light",
        })
    );

    Ok(())
}

#[tokio::test]
async fn scenes_create_serializes_mutation_mode_and_enabled() -> Result<()> {
    let captured_body: SharedBody = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route("/api/v1/scenes", post(capture_scene_create))
        .with_state(Arc::clone(&captured_body));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let cli_result = run_hyper(
        port,
        &[
            "scenes",
            "create",
            "movie-night",
            "--description",
            "Cozy lights",
            "--mutation-mode",
            "snapshot",
            "--enabled",
            "false",
        ],
    )
    .await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    cli_result?;

    let body = captured_body
        .lock()
        .await
        .clone()
        .context("server did not capture scene create request body")?;
    assert_eq!(
        body,
        serde_json::json!({
            "name": "movie-night",
            "description": "Cozy lights",
            "enabled": false,
            "mutation_mode": "snapshot",
        })
    );

    Ok(())
}

#[tokio::test]
async fn scenes_activate_sends_transition_ms_body() -> Result<()> {
    let captured_body: SharedBody = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route(
            "/api/v1/scenes/{scene}/activate",
            post(capture_scene_activate),
        )
        .with_state(Arc::clone(&captured_body));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let cli_result = run_hyper(
        port,
        &["scenes", "activate", "movie-night", "--transition", "250"],
    )
    .await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    cli_result?;

    let body = captured_body
        .lock()
        .await
        .clone()
        .context("server did not capture scene activate request body")?;
    assert_eq!(body, serde_json::json!({ "transition_ms": 250 }));

    Ok(())
}

#[tokio::test]
async fn scenes_deactivate_sends_empty_object_body() -> Result<()> {
    let captured_body: SharedBody = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route("/api/v1/scene/deactivate", post(capture_scene_deactivate))
        .with_state(Arc::clone(&captured_body));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let cli_result = run_hyper(port, &["scenes", "deactivate"]).await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    cli_result?;

    let body = captured_body
        .lock()
        .await
        .clone()
        .context("server did not capture scene deactivate request body")?;
    assert_eq!(body, serde_json::json!({}));

    Ok(())
}

/// The zone resource every scene write answers with, in the shape the
/// daemon actually publishes.
fn zone_resource_fixture(zone_id: &str, layer_id: &str, effect_id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": zone_id,
        "name": "Primary",
        "description": null,
        "role": "primary",
        "enabled": true,
        "brightness": 1.0,
        "color": null,
        "display_target": null,
        "members": [],
        "layout": null,
        "layers": [{
            "id": layer_id,
            "source": {
                "type": "effect",
                "effect_id": effect_id,
                "controls": {}
            },
            "blend": "replace",
            "opacity": 1.0
        }]
    })
}

async fn capture_effect_apply(
    Path(_effect): Path<String>,
    State(captured_body): State<SharedBody>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    *captured_body.lock().await = Some(body);
    Json(serde_json::json!({
        "data": {
            "zone": zone_resource_fixture(
                "00000000-0000-0000-0000-000000000010",
                "00000000-0000-0000-0000-000000000011",
                "00000000-0000-0000-0000-000000000012",
            ),
            "transition": { "type": "cut" },
            "output": { "applied": true },
        },
    }))
}

async fn effect_detail_with_color(Path(effect): Path<String>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "data": {
            "id": effect,
            "name": "Demo",
            "description": "test",
            "author": "test",
            "category": "ambient",
            "source": "native",
            "runnable": true,
            "tags": [],
            "version": "1",
            "audio_reactive": false,
            "controls": [{
                "id": "accent",
                "name": "Accent",
                "kind": "color",
                "control_type": "color_picker",
                "default_value": {
                    "kind": "color_linear",
                    "value": {"r": 1.0, "g": 1.0, "b": 1.0, "a": 1.0}
                }
            }]
        }
    }))
}

/// One stored scene in the shape `GET`/`POST /scenes` publishes.
fn scene_summary_fixture(id: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "description": null,
        "enabled": true,
        "priority": 50,
        "mutation_mode": "live"
    })
}

/// The live scene document, as `GET /scene` publishes it.
fn scene_document_fixture() -> serde_json::Value {
    serde_json::json!({
        "id": "0198c5b6-3333-7000-8000-000000000001",
        "name": "Default",
        "description": null,
        "kind": "named",
        "is_default": true,
        "unassigned_behavior": "off",
        "layout_id": null,
        "activation_brightness": null,
        "priority": 50,
        "enabled": true,
        "metadata": {},
        "mutation_mode": "live",
        "revision": 7,
        "zones": []
    })
}

async fn capture_scene_create(
    State(captured_body): State<SharedBody>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    *captured_body.lock().await = Some(body);
    Json(serde_json::json!({
        "data": scene_summary_fixture("scene_movie_night", "Movie Night"),
    }))
}

async fn capture_scene_activate(
    Path(scene): Path<String>,
    State(captured_body): State<SharedBody>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    *captured_body.lock().await = Some(body);
    Json(serde_json::json!({
        "data": {
            "scene": { "id": scene, "name": "Movie Night" },
            "activated": true,
            "layout": { "layout_id": null, "applied": true },
            "brightness": { "applied": true },
        },
    }))
}

async fn capture_scene_deactivate(
    State(captured_body): State<SharedBody>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    *captured_body.lock().await = Some(body);
    Json(serde_json::json!({
        "data": scene_document_fixture(),
    }))
}

async fn capture_control_patch(
    Path(surface_id): Path<String>,
    State((captured_uri, captured_body)): State<SharedRequest>,
    uri: Uri,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    assert_eq!(surface_id, "driver:wled");
    *captured_uri.lock().await = Some(uri.to_string());
    *captured_body.lock().await = Some(body);
    Json(serde_json::json!({
        "data": {
            "surface_id": "driver:wled",
            "previous_revision": 3,
            "revision": 4,
            "accepted": [{
                "field_id": "default_protocol",
                "value": { "kind": "enum", "value": "ddp" }
            }],
            "rejected": [],
            "impacts": [],
            "values": {
                "default_protocol": {
                    "kind": "enum",
                    "value": "ddp"
                }
            }
        }
    }))
}

async fn driver_control_surface(Path(driver): Path<String>) -> Json<serde_json::Value> {
    assert_eq!(driver, "wled");
    Json(driver_control_surface_response())
}

fn driver_control_surface_response() -> serde_json::Value {
    serde_json::json!({
        "data": {
            "surface_id": "driver:wled",
            "scope": {
                "driver": {
                    "driver_id": "wled"
                }
            },
            "schema_version": 1,
            "revision": 3,
            "groups": [],
            "fields": [],
            "actions": [],
            "values": {},
            "availability": {},
            "action_availability": {}
        }
    })
}

async fn capture_driver_control_surface(
    Path(driver): Path<String>,
    State(captured_uri): State<SharedUri>,
    uri: Uri,
) -> Json<serde_json::Value> {
    assert_eq!(driver, "wled");
    *captured_uri.lock().await = Some(uri.to_string());
    Json(driver_control_surface_response())
}

async fn confirmed_driver_control_surface(
    Path(driver): Path<String>,
    State(captured_uri): State<SharedUri>,
    uri: Uri,
) -> Json<serde_json::Value> {
    assert_eq!(driver, "wled");
    *captured_uri.lock().await = Some(uri.to_string());
    let mut response = driver_control_surface_response();
    response["data"]["actions"] = serde_json::json!([{
        "id": "factory_reset",
        "label": "Factory reset",
        "description": null,
        "group_id": null,
        "input_fields": [],
        "result_type": null,
        "confirmation": {
            "level": "destructive",
            "message": "Factory reset this driver?"
        },
        "apply_impact": "hardware_persist",
        "availability": { "kind": "always" },
        "ordering": 0,
        "owner": {
            "driver": {
                "driver_id": "wled"
            }
        }
    }]);
    Json(response)
}

async fn capture_control_action(
    Path((surface_id, action_id)): Path<(String, String)>,
    State((captured_uri, captured_body)): State<SharedRequest>,
    uri: Uri,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    assert_eq!(surface_id, "driver:wled");
    assert_eq!(action_id, "rescan");
    *captured_uri.lock().await = Some(uri.to_string());
    *captured_body.lock().await = Some(body);
    Json(serde_json::json!({
        "data": {
            "surface_id": "driver:wled",
            "action_id": "rescan",
            "status": "completed",
            "result": null,
            "revision": 4
        }
    }))
}

async fn capture_device_control_patch(
    Path(surface_id): Path<String>,
    State((captured_uri, captured_body)): State<SharedRequest>,
    uri: Uri,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    assert_eq!(
        surface_id,
        format!("driver:wled:device:{}", test_device_id())
    );
    *captured_uri.lock().await = Some(uri.to_string());
    *captured_body.lock().await = Some(body);
    Json(serde_json::json!({
        "data": {
            "surface_id": format!("driver:wled:device:{}", test_device_id()),
            "previous_revision": 2,
            "revision": 3,
            "accepted": [{
                "field_id": "color_order",
                "value": { "kind": "enum", "value": "grb" }
            }],
            "rejected": [],
            "impacts": [],
            "values": {
                "color_order": {
                    "kind": "enum",
                    "value": "grb"
                }
            }
        }
    }))
}

fn device_control_surface_response() -> serde_json::Value {
    serde_json::json!({
        "data": {
            "surface_id": format!("device:{}", test_device_id()),
            "scope": {
                "device": {
                    "device_id": test_device_id(),
                    "driver_id": "host"
                }
            },
            "schema_version": 1,
            "revision": 2,
            "groups": [],
            "fields": [],
            "actions": [{
                "id": "identify",
                "owner": "host",
                "label": "Identify",
                "input_fields": [],
                "apply_impact": "live",
                "availability": { "kind": "always" },
                "ordering": 0
            }],
            "values": {},
            "availability": {},
            "action_availability": {}
        }
    })
}

fn driver_device_control_surface_response() -> serde_json::Value {
    serde_json::json!({
        "data": {
            "surface_id": format!("driver:wled:device:{}", test_device_id()),
            "scope": {
                "device": {
                    "device_id": test_device_id(),
                    "driver_id": "wled"
                }
            },
            "schema_version": 1,
            "revision": 2,
            "groups": [],
            "fields": [{
                "id": "color_order",
                "owner": { "driver": { "driver_id": "wled" } },
                "label": "Color order",
                "value_type": {
                    "kind": "enum",
                    "options": [{
                        "value": "grb",
                        "label": "GRB",
                        "deprecated": false
                    }]
                },
                "access": "read_write",
                "persistence": "device_config",
                "apply_impact": "live",
                "visibility": "standard",
                "availability": { "kind": "always" },
                "ordering": 0
            }],
            "actions": [],
            "values": {
                "color_order": {
                    "kind": "enum",
                    "value": "grb"
                }
            },
            "availability": {},
            "action_availability": {}
        }
    })
}

async fn capture_device_control_surface_list(
    Query(query): Query<std::collections::BTreeMap<String, String>>,
    State((captured_uri, _captured_body)): State<SharedRequest>,
    uri: Uri,
) -> Json<serde_json::Value> {
    assert_eq!(
        query.get("device_id").map(String::as_str),
        Some(test_device_id())
    );
    *captured_uri.lock().await = Some(uri.to_string());
    Json(serde_json::json!({
        "data": {
            "surfaces": [
                device_control_surface_response()["data"].clone(),
                driver_device_control_surface_response()["data"].clone()
            ]
        }
    }))
}

async fn capture_device_control_action(
    Path((surface_id, action_id)): Path<(String, String)>,
    State((captured_uri, captured_body)): State<SharedRequest>,
    uri: Uri,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    assert_eq!(surface_id, format!("device:{}", test_device_id()));
    assert_eq!(action_id, "identify");
    *captured_uri.lock().await = Some(uri.to_string());
    *captured_body.lock().await = Some(body);
    Json(serde_json::json!({
        "data": {
            "surface_id": format!("device:{}", test_device_id()),
            "action_id": "identify",
            "status": "completed",
            "result": null,
            "revision": 3
        }
    }))
}

async fn capture_scene_snapshot(
    State(captured_body): State<SharedBody>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    *captured_body.lock().await = Some(body);
    Json(serde_json::json!({
        "data": scene_summary_fixture("0198c5b6-1111-7000-8000-000000000005", "Evening"),
    }))
}

fn test_device_id() -> &'static str {
    "00000000-0000-0000-0000-000000000001"
}

type SharedUris = Arc<Mutex<Vec<String>>>;

fn cli_device_page(offset: u64, names: &[&str], has_more: bool) -> serde_json::Value {
    let items: Vec<serde_json::Value> = names
        .iter()
        .map(|name| {
            serde_json::json!({
                "id": name,
                "layout_device_id": name,
                "name": name,
                "origin": {
                    "driver_id": "wled",
                    "backend_id": "wled",
                    "transport": "network"
                },
                "presentation": { "label": "WLED" },
                "status": "connected",
                "brightness": 100,
                "total_leds": 30,
                "segments": []
            })
        })
        .collect();

    serde_json::json!({
        "data": {
            "items": items,
            "total": 3,
            "page": { "offset": offset, "limit": 200, "has_more": has_more }
        }
    })
}

#[tokio::test]
async fn devices_list_requests_the_route_ceiling_and_follows_has_more() -> Result<()> {
    let captured_uris: SharedUris = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new()
        .route(
            "/api/v1/devices",
            get(|State(seen): State<SharedUris>, uri: Uri| async move {
                let query = uri.query().unwrap_or_default().to_owned();
                let first_page = !query.contains("offset=2");
                seen.lock().await.push(query);
                if first_page {
                    Json(cli_device_page(0, &["desk", "shelf"], true))
                } else {
                    Json(cli_device_page(2, &["ceiling"], false))
                }
            }),
        )
        .with_state(Arc::clone(&captured_uris));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let cli_output = run_hyper_output(port, &["devices", "list"]).await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    let cli_output = cli_output?;
    assert!(cli_output.status.success());

    assert_eq!(
        captured_uris.lock().await.as_slice(),
        ["limit=200&offset=0", "limit=200&offset=2"]
    );

    let rendered: serde_json::Value = serde_json::from_slice(&cli_output.stdout)
        .context("devices list --json should emit one JSON document")?;
    let items = rendered["items"]
        .as_array()
        .context("merged listing should carry items")?;
    assert_eq!(items.len(), 3);
    assert_eq!(items[2]["name"], "ceiling");
    assert_eq!(rendered["total"], 3);

    Ok(())
}

#[tokio::test]
async fn layouts_list_requests_the_route_ceiling() -> Result<()> {
    let captured_uris: SharedUris = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new()
        .route(
            "/api/v1/layouts",
            get(|State(seen): State<SharedUris>, uri: Uri| async move {
                seen.lock()
                    .await
                    .push(uri.query().unwrap_or_default().to_owned());
                Json(serde_json::json!({
                    "data": {
                        "items": [{
                            "id": "desk",
                            "name": "Desk",
                            "canvas_width": 640,
                            "canvas_height": 480,
                            "zone_count": 2
                        }],
                        "total": 1,
                        "page": { "offset": 0, "limit": 200, "has_more": false }
                    }
                }))
            }),
        )
        .with_state(Arc::clone(&captured_uris));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let cli_result = run_hyper(port, &["layouts", "list"]).await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    cli_result?;

    assert_eq!(
        captured_uris.lock().await.as_slice(),
        ["limit=200&offset=0"]
    );

    Ok(())
}

/// Decode one `--json` invocation's stdout.
async fn run_hyper_json(port: u16, args: &[&str]) -> Result<serde_json::Value> {
    let output = run_hyper_output(port, args).await?;
    if !output.status.success() {
        bail!(
            "hyper CLI failed (status={}):\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    serde_json::from_slice(&output.stdout).context("CLI --json output was not valid JSON")
}

#[tokio::test]
async fn effects_list_json_output_preserves_the_daemon_payload() -> Result<()> {
    let payload = serde_json::json!({
        "items": [{
            "id": "aurora",
            "name": "Aurora",
            "description": "Slow ribbons",
            "author": "hyperb1iss",
            "category": "ambient",
            "source": "html",
            "runnable": true,
            "tags": ["calm"],
            "version": "1.2.0",
            "audio_reactive": false,
            "input_reactive": false,
            "capabilities": {
                "audio_reactive": false,
                "screen_reactive": false,
                "input_reactive": false
            }
        }],
        "total": 1
    });
    let expected = payload.clone();
    let router = Router::new().route(
        "/api/v1/effects",
        get(move || {
            let payload = payload.clone();
            async move { Json(serde_json::json!({ "data": payload })) }
        }),
    );
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let rendered = run_hyper_json(port, &["effects", "list"]).await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;

    assert_eq!(rendered?, expected);

    Ok(())
}

#[tokio::test]
async fn scenes_list_json_output_preserves_the_daemon_payload() -> Result<()> {
    let payload = serde_json::json!({
        "items": [
            scene_summary_fixture("0198c5b6-4444-7000-8000-000000000001", "Movie Night"),
            scene_summary_fixture("0198c5b6-4444-7000-8000-000000000002", "Focus")
        ],
        "total": 2
    });
    let expected = payload.clone();
    let router = Router::new().route(
        "/api/v1/scenes",
        get(move || {
            let payload = payload.clone();
            async move { Json(serde_json::json!({ "data": payload })) }
        }),
    );
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let rendered = run_hyper_json(port, &["scenes", "list"]).await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;

    assert_eq!(rendered?, expected);

    Ok(())
}

#[tokio::test]
async fn layouts_show_json_output_preserves_the_daemon_payload() -> Result<()> {
    let payload = serde_json::json!({
        "id": "desk",
        "name": "Desk",
        "description": null,
        "canvas_width": 640,
        "canvas_height": 480,
        "zones": [],
        "default_sampling_mode": { "type": "bilinear" },
        "default_edge_behavior": "clamp",
        "spaces": null,
        "version": 1
    });
    let expected = payload.clone();
    let router = Router::new().route(
        "/api/v1/layouts/{id}",
        get(move |Path(id): Path<String>| {
            let payload = payload.clone();
            async move {
                assert_eq!(id, "desk");
                Json(serde_json::json!({ "data": payload }))
            }
        }),
    );
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let rendered = run_hyper_json(port, &["layouts", "show", "desk"]).await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;

    assert_eq!(rendered?, expected);

    Ok(())
}

#[tokio::test]
async fn devices_list_json_output_preserves_the_daemon_payload() -> Result<()> {
    let payload = serde_json::json!({
        "items": [{
            "id": test_device_id(),
            "layout_device_id": test_device_id(),
            "name": "Desk Strip",
            "origin": {
                "driver_id": "wled",
                "backend_id": "wled",
                "transport": "network"
            },
            "presentation": { "label": "WLED" },
            "status": "connected",
            "brightness": 80,
            "firmware_version": "0.15.0",
            "connection": {
                "transport": "network",
                "label": null,
                "endpoint": null,
                "ip": "10.0.0.4",
                "hostname": null
            },
            "total_leds": 144,
            "segments": []
        }],
        "total": 1
    });
    let expected = payload.clone();
    let router = Router::new().route(
        "/api/v1/devices",
        get(move || {
            let payload = payload.clone();
            async move { Json(serde_json::json!({ "data": payload })) }
        }),
    );
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let rendered = run_hyper_json(port, &["devices", "list"]).await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;

    assert_eq!(rendered?, expected);

    Ok(())
}

#[tokio::test]
async fn controls_show_json_output_preserves_the_daemon_payload() -> Result<()> {
    let expected = driver_control_surface_response()["data"].clone();
    let router = Router::new().route(
        "/api/v1/drivers/{driver}/controls",
        get(|Path(driver): Path<String>| async move {
            assert_eq!(driver, "wled");
            Json(driver_control_surface_response())
        }),
    );
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let rendered = run_hyper_json(port, &["controls", "show", "driver:wled"]).await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;

    assert_eq!(rendered?, expected);

    Ok(())
}

#[tokio::test]
async fn drivers_list_json_output_preserves_the_daemon_payload() -> Result<()> {
    let payload = serde_json::json!({
        "items": [{
            "descriptor": {
                "id": "wled",
                "display_name": "WLED",
                "module_kind": "network",
                "transports": ["network"],
                "capabilities": {
                    "config": true,
                    "discovery": true,
                    "pairing": false,
                    "output_backend": true,
                    "protocol_catalog": false,
                    "runtime_cache": false,
                    "credentials": false,
                    "presentation": true,
                    "controls": true
                },
                "api_schema_version": 1,
                "config_version": 1,
                "default_enabled": true
            },
            "presentation": { "label": "WLED" },
            "enabled": true,
            "config_key": "wled",
            "protocols": [],
            "control_surface_id": "driver:wled"
        }]
    });
    let expected = payload.clone();
    let router = Router::new().route(
        "/api/v1/drivers",
        get(move || {
            let payload = payload.clone();
            async move { Json(serde_json::json!({ "data": payload })) }
        }),
    );
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let rendered = run_hyper_json(port, &["drivers", "list"]).await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;

    assert_eq!(rendered?, expected);

    Ok(())
}

#[tokio::test]
async fn library_presets_list_json_output_preserves_the_daemon_payload() -> Result<()> {
    let payload = serde_json::json!({
        "items": [{
            "id": "0198c5b6-5555-7000-8000-000000000001",
            "name": "Warm Desk",
            "description": null,
            "effect_id": "0198c5b6-5555-7000-8000-000000000002",
            "controls": { "speed": { "kind": "float", "value": 0.25 } },
            "tags": ["warm"],
            "created_at_ms": 1_700_000_000_000_u64,
            "updated_at_ms": 1_700_000_500_000_u64
        }],
        "total": 1
    });
    let expected = payload.clone();
    let router = Router::new().route(
        "/api/v1/library/presets",
        get(move || {
            let payload = payload.clone();
            async move { Json(serde_json::json!({ "data": payload })) }
        }),
    );
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let rendered = run_hyper_json(port, &["library", "presets", "list"]).await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;

    assert_eq!(rendered?, expected);

    Ok(())
}

/// The daemon's system resource, built from the canonical contract so the
/// fixture cannot drift from the shape the daemon serializes.
fn system_resource_fixture() -> serde_json::Value {
    let resource = SystemResource {
        identity: ServerInfo {
            instance_id: "0198c5b6-6666-7000-8000-000000000001".to_owned(),
            instance_name: "studio".to_owned(),
            version: "0.9.0".to_owned(),
            server_session_id: None,
            device_count: 2,
            auth_required: false,
        },
        status: Some(SystemStatus {
            running: true,
            version: "0.9.0".to_owned(),
            uptime_seconds: 3_661,
            device_count: 2,
            effect_count: 18,
            scene_count: 4,
            active_effect: Some("Aurora".to_owned()),
            active_scene: Some("Movie Night".to_owned()),
            global_brightness: 80,
            audio_available: true,
            capabilities: vec!["effects".to_owned(), "scenes".to_owned()],
            ..SystemStatus::default()
        }),
    };
    serde_json::to_value(resource).expect("the system resource should serialize")
}

#[tokio::test]
async fn status_json_output_preserves_the_daemon_status_payload() -> Result<()> {
    let expected = system_resource_fixture()["status"].clone();
    let router = Router::new().route(
        "/api/v1/system",
        get(|| async { Json(serde_json::json!({ "data": system_resource_fixture() })) }),
    );
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let rendered = run_hyper_json(port, &["status"]).await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;

    assert_eq!(rendered?, expected);

    Ok(())
}

#[tokio::test]
async fn audio_devices_reads_the_daemon_device_list() -> Result<()> {
    let router = Router::new().route(
        "/api/v1/system/audio-devices",
        get(|| async {
            Json(serde_json::json!({
                "data": {
                    "devices": [
                        { "id": "default", "name": "System default", "description": "Follows the host" },
                        { "id": "hw:1", "name": "Scarlett 2i2", "description": "USB interface" }
                    ],
                    "current": "hw:1"
                }
            }))
        }),
    );
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let stdout = run_hyper_plain(port, &["audio", "devices"]).await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;

    let stdout = stdout?;
    assert!(
        stdout.contains("System default") && stdout.contains("Scarlett 2i2"),
        "every enumerated device should render, got: {stdout}"
    );

    Ok(())
}

#[tokio::test]
async fn diagnose_json_output_preserves_the_whole_daemon_report() -> Result<()> {
    let payload = serde_json::json!({
        "checks": [{
            "category": "daemon",
            "name": "daemon_running",
            "status": "pass",
            "detail": "up 1h"
        }],
        "summary": { "passed": 1, "warnings": 0, "failed": 0 },
        "snapshot": {
            "input": { "keyboard_events": 42 },
            "render": { "elapsed_ms": 3_600_000.0 }
        }
    });
    let expected = payload.clone();
    let router = Router::new().route(
        "/api/v1/diagnose",
        post(move |Json(_body): Json<serde_json::Value>| {
            let payload = payload.clone();
            async move { Json(serde_json::json!({ "data": payload })) }
        }),
    );
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let rendered = run_hyper_json(port, &["diagnose"]).await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;

    assert_eq!(
        rendered?, expected,
        "the daemon-local snapshot section must survive the CLI untouched"
    );

    Ok(())
}

#[tokio::test]
async fn config_get_prints_the_daemon_key_value() -> Result<()> {
    let router = Router::new().route(
        "/api/v1/config/keys/{key}",
        get(|Path(key): Path<String>| async move {
            assert_eq!(key, "daemon.port");
            Json(serde_json::json!({
                "data": { "key": "daemon.port", "value": 9420 }
            }))
        }),
    );
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let stdout = run_hyper_plain(port, &["config", "get", "daemon.port"]).await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;

    assert_eq!(stdout?.trim(), "9420");

    Ok(())
}
