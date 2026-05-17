//! User media asset endpoints — `/api/v1/assets`.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use hypercolor_core::asset::{
    AssetEvent, AssetLibraryError, AssetTypeHint, AssetUploadOptions, MediaAssetRecord,
};
use hypercolor_types::asset::AssetId;
use hypercolor_types::event::{AssetChangeKind, HypercolorEvent};
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};

#[derive(Debug, Serialize)]
pub struct AssetListResponse {
    pub items: Vec<MediaAssetRecord>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct AssetUploadResponse {
    #[serde(flatten)]
    pub record: MediaAssetRecord,
    pub duplicate: bool,
}

#[derive(Debug, Deserialize)]
pub struct AssetUploadQuery {
    #[serde(default)]
    pub rename_duplicate: bool,
    #[serde(default)]
    pub r#type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AssetUpdateRequest {
    pub name: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug)]
struct ParsedUpload {
    bytes: Vec<u8>,
    name: String,
    tags: Vec<String>,
    type_hint: Option<AssetTypeHint>,
}

pub async fn list_assets(State(state): State<Arc<AppState>>) -> Response {
    let library = state.asset_library.read().await;
    let items = library.records().to_vec();
    ApiResponse::ok(AssetListResponse {
        total: items.len(),
        items,
    })
}

pub async fn get_asset(State(state): State<Arc<AppState>>, Path(id): Path<AssetId>) -> Response {
    let library = state.asset_library.read().await;
    let Some(record) = library.get(id).cloned() else {
        return ApiError::not_found(format!("Asset not found: {id}"));
    };
    ApiResponse::ok(record)
}

pub async fn upload_asset(
    State(state): State<Arc<AppState>>,
    Query(query): Query<AssetUploadQuery>,
    multipart: Multipart,
) -> Response {
    let parsed = match parse_upload(multipart, query.r#type.as_deref()).await {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    let mut options = AssetUploadOptions::new(parsed.name);
    options.tags = parsed.tags;
    options.type_hint = parsed.type_hint;
    options.rename_duplicate = query.rename_duplicate;

    let upsert = {
        let mut library = state.asset_library.write().await;
        match library.add_bytes(&parsed.bytes, options) {
            Ok(upsert) => upsert,
            Err(error) => return asset_error_response(error),
        }
    };

    publish_asset_events(state.as_ref(), &upsert.events);
    let response = AssetUploadResponse {
        record: upsert.record,
        duplicate: upsert.duplicate,
    };
    if upsert.duplicate {
        ApiResponse::ok(response)
    } else {
        ApiResponse::created(response)
    }
}

pub async fn update_asset(
    State(state): State<Arc<AppState>>,
    Path(id): Path<AssetId>,
    axum::Json(body): axum::Json<AssetUpdateRequest>,
) -> Response {
    let update = {
        let mut library = state.asset_library.write().await;
        match library.update_metadata(id, body.name, body.tags) {
            Ok(Some(update)) => update,
            Ok(None) => return ApiError::not_found(format!("Asset not found: {id}")),
            Err(error) => return asset_error_response(error),
        }
    };

    if let Some(event) = &update.event {
        publish_asset_events(state.as_ref(), std::slice::from_ref(event));
    }
    ApiResponse::ok(update.record)
}

pub async fn delete_asset(State(state): State<Arc<AppState>>, Path(id): Path<AssetId>) -> Response {
    let event = {
        let mut library = state.asset_library.write().await;
        match library.remove(id) {
            Ok(Some(event)) => event,
            Ok(None) => return ApiError::not_found(format!("Asset not found: {id}")),
            Err(error) => return asset_error_response(error),
        }
    };

    publish_asset_events(state.as_ref(), std::slice::from_ref(&event));
    ApiResponse::ok(serde_json::json!({ "removed": id }))
}

pub async fn get_asset_blob(
    State(state): State<Arc<AppState>>,
    Path(id): Path<AssetId>,
) -> Response {
    let (record, path) = {
        let library = state.asset_library.read().await;
        let Some(record) = library.get(id).cloned() else {
            return ApiError::not_found(format!("Asset not found: {id}"));
        };
        let path = match library.object_path_for_hash(&record.hash_sha256) {
            Ok(path) => path,
            Err(error) => return asset_error_response(error),
        };
        (record, path)
    };

    match tokio::fs::read(&path).await {
        Ok(bytes) => binary_response(record.mime_type, bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ApiError::not_found(format!("Asset blob not found: {id}"))
        }
        Err(error) => ApiError::internal(format!("Failed to read asset blob: {error}")),
    }
}

pub async fn get_asset_thumbnail(
    State(state): State<Arc<AppState>>,
    Path(id): Path<AssetId>,
) -> Response {
    let path = {
        let library = state.asset_library.read().await;
        if !library.contains(id) {
            return ApiError::not_found(format!("Asset not found: {id}"));
        }
        library.thumbnail_path(id)
    };

    match tokio::fs::read(&path).await {
        Ok(bytes) => binary_response("image/webp".to_owned(), bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ApiError::not_found(format!("Asset thumbnail not found: {id}"))
        }
        Err(error) => ApiError::internal(format!("Failed to read asset thumbnail: {error}")),
    }
}

async fn parse_upload(
    mut multipart: Multipart,
    query_type_hint: Option<&str>,
) -> Result<ParsedUpload, Response> {
    let mut file_bytes = None;
    let mut file_name = None;
    let mut display_name = None;
    let mut tags = Vec::new();
    let mut type_hint = parse_type_hint(query_type_hint).map_err(ApiError::bad_request)?;

    while let Some(field) = multipart.next_field().await.map_err(|error| {
        ApiError::bad_request(format!("Failed to read multipart upload: {error}"))
    })? {
        let field_name = field.name().map(ToOwned::to_owned);
        match field_name.as_deref() {
            Some("file") => {
                file_name = field.file_name().map(ToOwned::to_owned);
                let bytes = field.bytes().await.map_err(|error| {
                    ApiError::bad_request(format!("Failed to read uploaded file: {error}"))
                })?;
                file_bytes = Some(bytes.to_vec());
            }
            Some("name") => {
                display_name = Some(field.text().await.map_err(|error| {
                    ApiError::bad_request(format!("Failed to read asset name: {error}"))
                })?);
            }
            Some("tags") => {
                let raw = field.text().await.map_err(|error| {
                    ApiError::bad_request(format!("Failed to read asset tags: {error}"))
                })?;
                tags = parse_tags(&raw).map_err(ApiError::bad_request)?;
            }
            Some("type") => {
                let raw = field.text().await.map_err(|error| {
                    ApiError::bad_request(format!("Failed to read asset type hint: {error}"))
                })?;
                type_hint = parse_type_hint(Some(&raw)).map_err(ApiError::bad_request)?;
            }
            _ => {}
        }
    }

    let Some(bytes) = file_bytes else {
        return Err(ApiError::bad_request(
            "Missing multipart file field named \"file\".",
        ));
    };
    let name = display_name
        .filter(|name| !name.trim().is_empty())
        .or(file_name)
        .unwrap_or_else(|| "asset".to_owned());

    Ok(ParsedUpload {
        bytes,
        name,
        tags,
        type_hint,
    })
}

fn parse_type_hint(raw: Option<&str>) -> Result<Option<AssetTypeHint>, String> {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    if raw.eq_ignore_ascii_case("lottie") {
        return Ok(Some(AssetTypeHint::Lottie));
    }
    if raw.eq_ignore_ascii_case("stream") || raw.eq_ignore_ascii_case("livestream") {
        return Ok(Some(AssetTypeHint::Stream));
    }
    Err(format!("Unsupported asset type hint: {raw}"))
}

fn parse_tags(raw: &str) -> Result<Vec<String>, String> {
    if raw.trim_start().starts_with('[') {
        return serde_json::from_str::<Vec<String>>(raw)
            .map_err(|error| format!("Invalid JSON tags array: {error}"));
    }
    Ok(raw
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn asset_error_response(error: AssetLibraryError) -> Response {
    match error {
        AssetLibraryError::HardCapExceeded {
            byte_len,
            hard_cap_bytes,
        } => ApiError::payload_too_large(format!(
            "Uploaded asset exceeds the hard cap ({byte_len} bytes > {hard_cap_bytes} bytes)."
        )),
        AssetLibraryError::UnsupportedMediaType { reason } => {
            ApiError::unsupported_media_type(reason)
        }
        AssetLibraryError::DecodeImage(error) => {
            ApiError::validation(format!("Failed to decode image asset: {error}"))
        }
        AssetLibraryError::InvalidHashPath { .. } => ApiError::internal(error.to_string()),
        AssetLibraryError::CreateDir { .. }
        | AssetLibraryError::Read { .. }
        | AssetLibraryError::Write { .. }
        | AssetLibraryError::Replace { .. }
        | AssetLibraryError::Sync { .. }
        | AssetLibraryError::ParseIndex { .. }
        | AssetLibraryError::SerializeIndex(_)
        | AssetLibraryError::EncodeThumbnail { .. } => ApiError::internal(error.to_string()),
        AssetLibraryError::NotFound(id) => ApiError::not_found(format!("Asset not found: {id}")),
    }
}

fn publish_asset_events(state: &AppState, events: &[AssetEvent]) {
    for event in events {
        let (asset_id, kind) = match event {
            AssetEvent::Added { record } => (record.id, AssetChangeKind::Added),
            AssetEvent::Modified { record } => (record.id, AssetChangeKind::Modified),
            AssetEvent::Removed { asset_id } => (*asset_id, AssetChangeKind::Removed),
        };
        state
            .event_bus
            .publish(HypercolorEvent::AssetChanged { asset_id, kind });
    }
}

fn binary_response(content_type: String, bytes: Vec<u8>) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );
    (headers, Bytes::from(bytes)).into_response()
}
