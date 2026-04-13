//! Integration tests for CLI request payloads sent to the daemon API.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axum::extract::{Path, State};
use axum::{Json, Router, routing::post};
use tokio::sync::{Mutex, oneshot};

type SharedBody = Arc<Mutex<Option<serde_json::Value>>>;

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
