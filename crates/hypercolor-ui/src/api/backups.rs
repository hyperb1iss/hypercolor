//! Backup transfers retain browser-owned file handles and stream media chunks.

use super::{ApiError, ApiResult, client};
pub use hypercolor_types::api::backups::{BackupApplyRequest, BackupApplyResponse, BackupPreview};

pub async fn apply(operation_id: uuid::Uuid) -> ApiResult<BackupApplyResponse> {
    client::post_json(
        "/api/v1/backups/apply",
        &BackupApplyRequest { operation_id },
    )
    .await
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use super::super::{
        browser_body::BrowserBlobSource,
        http_transport::{
            HttpBody, HttpBodySource, HttpCancellation, HttpHeader, HttpMethod, HttpStreamError,
            HttpStreamRequest,
        },
    };
    use super::*;
    use hypercolor_types::api::ApiResponse;
    use std::{
        num::NonZeroUsize,
        task::{Context, Poll},
    };
    use wasm_bindgen::{JsValue, prelude::wasm_bindgen};
    use wasm_bindgen_futures::JsFuture;

    #[wasm_bindgen(inline_js = r#"
export function chooseBackupFile() {
  if (!window.showSaveFilePicker) return Promise.resolve(null);
  return window.showSaveFilePicker({suggestedName: 'hypercolor.hypercolor-backup',
    types: [{description: 'Hypercolor backup', accept: {'application/x-tar': ['.hypercolor-backup']}}]});
}
export async function saveBackupFile(stream, destination, signal) {
  if (destination) {
    const writer = await destination.createWritable();
    await stream.pipeTo(writer, {signal});
    return;
  }
  const blob = await new Response(stream.pipeThrough(new TransformStream(), {signal})).blob();
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = 'hypercolor.hypercolor-backup';
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 60000);
}
"#)]
    extern "C" {
        #[wasm_bindgen(js_name = chooseBackupFile)]
        fn choose_backup_file() -> js_sys::Promise;
        #[wasm_bindgen(js_name = saveBackupFile)]
        fn save_backup_file(
            stream: web_sys::ReadableStream,
            destination: JsValue,
            signal: web_sys::AbortSignal,
        ) -> js_sys::Promise;
    }

    struct TransferGuard {
        cancellation: Option<HttpCancellation>,
        controller: web_sys::AbortController,
    }
    impl Drop for TransferGuard {
        fn drop(&mut self) {
            if let Some(cancellation) = &self.cancellation {
                cancellation.cancel();
                self.controller.abort();
            }
        }
    }

    pub async fn export() -> ApiResult<()> {
        // Invoke the picker while the button's user activation is still live.
        let destination = JsFuture::from(choose_backup_file())
            .await
            .map_err(js_error)?;
        let response = client::send_stream(HttpStreamRequest {
            method: HttpMethod::Get,
            path: "/api/v1/backups/export".into(),
            headers: Vec::new(),
            body: HttpBody::new(Box::new(Empty), HttpCancellation::new()),
        })
        .await?;
        if !(200..300).contains(&response.status) {
            return Err(ApiError::Network(format!(
                "Backup download failed ({})",
                response.status
            )));
        }
        let mut transfer = TransferGuard {
            cancellation: Some(response.body.cancellation()),
            controller: web_sys::AbortController::new().map_err(js_error)?,
        };
        let stream = futures_util::stream::unfold(response.body, |mut body| async move {
            match body
                .read_chunk(NonZeroUsize::new(64 * 1024).expect("nonzero chunk"))
                .await
            {
                Ok(Some(bytes)) => Some((
                    Ok(JsValue::from(js_sys::Uint8Array::from(bytes.as_slice()))),
                    body,
                )),
                Ok(None) => None,
                Err(error) => Some((Err(JsValue::from_str(&error.to_string())), body)),
            }
        });
        let raw = wasm_streams::ReadableStream::from_stream(stream).into_raw();
        JsFuture::from(save_backup_file(
            raw,
            destination,
            transfer.controller.signal(),
        ))
        .await
        .map_err(js_error)?;
        transfer.cancellation.take();
        Ok(())
    }

    pub async fn preview(file: web_sys::File) -> ApiResult<BackupPreview> {
        let source = BrowserBlobSource::new(file.into()).map_err(stream_error)?;
        let response = client::send_stream(HttpStreamRequest {
            method: HttpMethod::Post,
            path: "/api/v1/backups/import".into(),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/octet-stream".into(),
            }],
            body: HttpBody::new(Box::new(source), HttpCancellation::new()),
        })
        .await?;
        let status = response.status;
        let chunk = NonZeroUsize::new(64 * 1024).expect("nonzero JSON chunk");
        if !(200..300).contains(&status) {
            let error: hypercolor_types::api::ApiErrorBody = response
                .body
                .collect_json(usize::MAX, chunk)
                .await
                .map_err(stream_error)?;
            return Err(ApiError::Network(error.error.message));
        }
        let value: ApiResponse<BackupPreview> = response
            .body
            .collect_json(usize::MAX, chunk)
            .await
            .map_err(stream_error)?;
        Ok(value.data)
    }

    struct Empty;
    impl HttpBodySource for Empty {
        fn exact_length(&self) -> Option<u64> {
            Some(0)
        }
        fn poll_chunk(
            &mut self,
            _: &mut Context<'_>,
            _: NonZeroUsize,
        ) -> Poll<Result<Option<Vec<u8>>, HttpStreamError>> {
            Poll::Ready(Ok(None))
        }
        fn cancel(&mut self) {}
    }
    fn stream_error(error: HttpStreamError) -> ApiError {
        ApiError::Network(error.to_string())
    }
    fn js_error(error: JsValue) -> ApiError {
        ApiError::Network(format!("Backup transfer: {error:?}"))
    }
}

#[cfg(target_arch = "wasm32")]
pub use browser::{export, preview};

#[cfg(not(target_arch = "wasm32"))]
pub async fn export() -> ApiResult<()> {
    Err(ApiError::Network(
        "Backup downloads require a browser".into(),
    ))
}
#[cfg(not(target_arch = "wasm32"))]
pub async fn preview(_: web_sys::File) -> ApiResult<BackupPreview> {
    Err(ApiError::Network("Backup uploads require a browser".into()))
}
