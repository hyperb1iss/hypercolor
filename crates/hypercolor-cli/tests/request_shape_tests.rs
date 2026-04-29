//! Integration tests for CLI request payloads sent to the daemon API.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axum::extract::{Path, State};
use axum::http::Uri;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use tokio::sync::{Mutex, oneshot};

type SharedBody = Arc<Mutex<Option<serde_json::Value>>>;
type SharedUri = Arc<Mutex<Option<String>>>;
type SharedRequest = (SharedUri, SharedBody);

async fn run_hyper(port: u16, args: &[&str]) -> Result<()> {
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
async fn effects_activate_serializes_scalar_params_and_default_cut_transition() -> Result<()> {
    let captured_body: SharedBody = Arc::new(Mutex::new(None));
    let router = Router::new()
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
    assert_eq!(body["controls"]["speed"], serde_json::json!(12.5));
    assert_eq!(body["controls"]["enabled"], serde_json::json!(true));
    assert_eq!(body["controls"]["label"], serde_json::json!("aurora"));
    assert_eq!(
        body["transition"],
        serde_json::json!({
            "type": "cut",
            "duration_ms": 0,
        })
    );

    Ok(())
}

#[tokio::test]
async fn controls_show_full_driver_device_surface_fetches_surface_by_id() -> Result<()> {
    let captured_uri: SharedUri = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route(
            "/api/v1/control-surfaces/{surface_id}",
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
            "/api/v1/control-surfaces/{surface_id}/values",
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
            "--expected-revision",
            "3",
            "--dry-run",
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
            "surface_id": "driver:wled",
            "changes": [{
                "field_id": "default_protocol",
                "value": {
                    "kind": "enum",
                    "value": "ddp"
                }
            }],
            "dry_run": true,
            "expected_revision": 3
        })
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
            "/api/v1/control-surfaces/{surface_id}/actions/{action_id}",
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
async fn devices_set_control_targets_device_surface() -> Result<()> {
    let captured_uri: SharedUri = Arc::new(Mutex::new(None));
    let captured_body: SharedBody = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route(
            "/api/v1/devices/{device}/controls",
            get(device_control_surface),
        )
        .route(
            "/api/v1/control-surfaces/{surface_id}/values",
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
            "--expected-revision",
            "2",
        ],
    )
    .await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    cli_result?;

    assert_eq!(
        captured_uri.lock().await.as_deref(),
        Some("/api/v1/control-surfaces/device%3A00000000-0000-0000-0000-000000000001/values")
    );
    assert_eq!(
        captured_body
            .lock()
            .await
            .clone()
            .context("server did not capture device control patch request body")?,
        serde_json::json!({
            "surface_id": "device:00000000-0000-0000-0000-000000000001",
            "changes": [{
                "field_id": "color_order",
                "value": {
                    "kind": "enum",
                    "value": "grb"
                }
            }],
            "dry_run": false,
            "expected_revision": 2
        })
    );

    Ok(())
}

#[tokio::test]
async fn devices_action_targets_device_surface() -> Result<()> {
    let captured_uri: SharedUri = Arc::new(Mutex::new(None));
    let captured_body: SharedBody = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route(
            "/api/v1/devices/{device}/controls",
            get(device_control_surface),
        )
        .route(
            "/api/v1/control-surfaces/{surface_id}/actions/{action_id}",
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
                    "kind": "duration_ms",
                    "value": 1200
                }
            }
        })
    );

    Ok(())
}

#[tokio::test]
async fn profiles_apply_sends_requested_transition_body() -> Result<()> {
    let captured_body: SharedBody = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route(
            "/api/v1/profiles/{profile}/apply",
            post(capture_profile_apply),
        )
        .with_state(Arc::clone(&captured_body));
    let (port, shutdown_tx, task) = spawn_server(router).await?;

    let cli_result = run_hyper(
        port,
        &["profiles", "apply", "evening", "--transition", "250"],
    )
    .await;

    let _ = shutdown_tx.send(());
    task.await.context("test server task join failed")?;
    cli_result?;

    let body = captured_body
        .lock()
        .await
        .clone()
        .context("server did not capture profile apply request body")?;
    assert_eq!(body, serde_json::json!({ "transition_ms": 250 }));

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
        .route("/api/v1/scenes/deactivate", post(capture_scene_deactivate))
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

async fn capture_effect_apply(
    Path(effect): Path<String>,
    State(captured_body): State<SharedBody>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    *captured_body.lock().await = Some(body);
    Json(serde_json::json!({
        "data": {
            "effect": {
                "id": effect,
                "name": "Demo Effect",
            },
        },
    }))
}

async fn capture_scene_create(
    State(captured_body): State<SharedBody>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    *captured_body.lock().await = Some(body);
    Json(serde_json::json!({
        "data": {
            "id": "scene_movie_night",
            "name": "Movie Night",
        },
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
            "activated": true,
            "scene": scene,
        },
    }))
}

async fn capture_scene_deactivate(
    State(captured_body): State<SharedBody>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    *captured_body.lock().await = Some(body);
    Json(serde_json::json!({
        "data": {
            "activated": true,
            "scene": "Default",
        },
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
            "accepted": ["default_protocol"],
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
    Json(serde_json::json!({
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
    }))
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
    assert_eq!(surface_id, format!("device:{}", test_device_id()));
    *captured_uri.lock().await = Some(uri.to_string());
    *captured_body.lock().await = Some(body);
    Json(serde_json::json!({
        "data": {
            "surface_id": format!("device:{}", test_device_id()),
            "previous_revision": 2,
            "revision": 3,
            "accepted": ["color_order"],
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

async fn device_control_surface(Path(device): Path<String>) -> Json<serde_json::Value> {
    assert_eq!(device, test_device_id());
    Json(serde_json::json!({
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
            "actions": [],
            "values": {},
            "availability": {},
            "action_availability": {}
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

async fn capture_profile_apply(
    Path(profile): Path<String>,
    State(captured_body): State<SharedBody>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    *captured_body.lock().await = Some(body);
    Json(serde_json::json!({
        "data": {
            "profile": {
                "id": profile,
                "name": "Evening",
            },
            "applied": true,
        },
    }))
}

fn test_device_id() -> &'static str {
    "00000000-0000-0000-0000-000000000001"
}
