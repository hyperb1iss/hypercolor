#![cfg(target_arch = "wasm32")]

use std::future::Future;
use std::num::NonZeroUsize;
use std::task::{Context, Waker};

use hypercolor_ui::api::browser_body::BrowserBlobSource;
use hypercolor_ui::api::http_transport::{
    HttpBody, HttpBodySource, HttpCancellation, HttpMultipartField, HttpMultipartSource,
    HttpMultipartValue, HttpStreamError,
};
use js_sys::{Array, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;
use web_sys::{Blob, BlobPropertyBag, File};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(inline_js = r#"
let originalRead, originalAbort, reads, aborts, readers, mode;
export function beginProbe(nextMode) {
  mode=nextMode; reads=[]; readers=[]; aborts=0;
  originalRead=FileReader.prototype.readAsArrayBuffer;
  originalAbort=FileReader.prototype.abort;
  FileReader.prototype.readAsArrayBuffer=function(blob) {
    reads.push(blob.size); readers.push(this);
    if(mode==='throw') throw new Error('injected read failure');
    return originalRead.call(this, mode==='short' ? blob.slice(0, Math.max(0,blob.size-1)) : blob);
  };
  FileReader.prototype.abort=function(){aborts++;return originalAbort.call(this)};
}
export function endProbe(){FileReader.prototype.readAsArrayBuffer=originalRead;FileReader.prototype.abort=originalAbort;}
export function readSizes(){return reads;}
export function abortCount(){return aborts;}
export function detached(){return readers.every(r=>r.onloadend===null);}
export function lateEvents(){for(const reader of readers)reader.dispatchEvent(new Event('loadend'));}
export async function finishPending(){const reader=readers.at(-1);if(reader.readyState!==FileReader.DONE)await new Promise(resolve=>reader.addEventListener('loadend',resolve,{once:true}));reader.dispatchEvent(new Event('loadend'));}
export function sizeOverride(blob,value){Object.defineProperty(blob,'size',{value});}
export function typedFile(blob,name){return new File([blob],name,{type:blob.type});}
export async function canonicalFields(bytes, contentType) {
 const fields=await new Response(bytes,{headers:{'Content-Type':contentType}}).formData();
 return JSON.stringify(await describe(fields));
}
async function describe(fields){const output=[];for(const [name,value] of fields)output.push(typeof value==='string'?{name,text:value}:{name,filename:value.name,mime:value.type,bytes:Array.from(new Uint8Array(await value.arrayBuffer()))});return output;}
export async function nativeFields(file){const fields=new FormData();fields.append('same\n"','one\rtwo');fields.append('same\n"',file,file.name);fields.append('same\n"','last');const encoded=new Response(fields);return JSON.stringify(await describe(await encoded.formData()));}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = beginProbe)]
    fn begin_probe(mode: &str);
    #[wasm_bindgen(js_name = endProbe)]
    fn end_probe();
    #[wasm_bindgen(js_name = readSizes)]
    fn read_sizes() -> Array;
    #[wasm_bindgen(js_name = abortCount)]
    fn abort_count() -> u32;
    fn detached() -> bool;
    #[wasm_bindgen(js_name = lateEvents)]
    fn late_events();
    #[wasm_bindgen(js_name = finishPending)]
    fn finish_pending() -> js_sys::Promise;
    #[wasm_bindgen(js_name = sizeOverride)]
    fn size_override(blob: &Blob, value: f64);
    #[wasm_bindgen(js_name = typedFile)]
    fn typed_file(blob: &Blob, name: &str) -> File;
    #[wasm_bindgen(js_name = canonicalFields)]
    fn canonical_fields(bytes: &Uint8Array, content_type: &str) -> js_sys::Promise;
    #[wasm_bindgen(js_name = nativeFields)]
    fn native_fields(file: &File) -> js_sys::Promise;
}
struct Probe;
impl Probe {
    fn new(mode: &str) -> Self {
        begin_probe(mode);
        Self
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        end_probe();
    }
}
fn maximum(size: usize) -> NonZeroUsize {
    NonZeroUsize::new(size).expect("positive")
}
fn blob(length: usize) -> Blob {
    let parts = Array::new();
    parts.push(&Uint8Array::from(vec![0x5a; length].as_slice()));
    Blob::new_with_u8_array_sequence(&parts).expect("Blob")
}
fn body(blob: Blob) -> HttpBody {
    HttpBody::new(
        Box::new(BrowserBlobSource::new(blob).expect("source")),
        HttpCancellation::new(),
    )
}

#[wasm_bindgen_test]
async fn blob_reads_only_credited_slices_and_preserves_bytes() {
    let _probe = Probe::new("normal");
    let mut body = body(blob(1024 * 1024));
    assert!(body.source_hint().expect("hint").is::<BrowserBlobSource>());
    assert_eq!(read_sizes().length(), 0);
    let mut total = 0;
    while let Some(chunk) = body.read_chunk(maximum(16384)).await.expect("chunk") {
        assert!(chunk.len() <= 16384);
        assert!(chunk.iter().all(|byte| *byte == 0x5a));
        total += chunk.len();
    }
    assert_eq!(total, 1024 * 1024);
    assert_eq!(read_sizes().length(), 64);
    assert!(
        read_sizes()
            .iter()
            .all(|size| size.as_f64() == Some(16384.0))
    );
    assert!(detached());
    assert_eq!(abort_count(), 0);
    assert!(body.source_hint().is_none());
}

#[wasm_bindgen_test]
async fn dropped_read_future_resumes_with_smaller_credit_without_a_second_read() {
    let _probe = Probe::new("normal");
    let mut body = body(blob(64));
    {
        let mut read = std::pin::pin!(body.read_chunk(maximum(64)));
        assert!(
            read.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
    }
    assert!(body.source_hint().is_none());
    JsFuture::from(finish_pending())
        .await
        .expect("read completed with duplicate loadend");
    for _ in 0..64 {
        assert_eq!(
            body.read_chunk(maximum(1)).await.expect("byte"),
            Some(vec![0x5a])
        );
    }
    assert_eq!(body.read_chunk(maximum(1)).await.expect("EOF"), None);
    assert_eq!(read_sizes().length(), 1);
    assert!(detached());
}

#[wasm_bindgen_test]
fn cancelling_pending_read_aborts_and_late_events_cannot_restore_it() {
    let _probe = Probe::new("normal");
    let mut body = body(blob(1024));
    {
        let mut read = std::pin::pin!(body.read_chunk(maximum(64)));
        assert!(
            read.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
    }
    body.cancel();
    assert_eq!(abort_count(), 1);
    assert!(detached());
    late_events();
    assert!(body.source_hint().is_none());
    drop(body);
    assert_eq!(abort_count(), 1);
}

#[wasm_bindgen_test]
async fn short_and_failed_reads_are_terminal() {
    for mode in ["short", "throw"] {
        let _probe = Probe::new(mode);
        let mut body = body(blob(20));
        let error = body
            .read_chunk(maximum(8))
            .await
            .expect_err("injected read failure");
        if mode == "short" {
            assert_eq!(error, HttpStreamError::LengthMismatch);
        }
        assert!(body.cancellation().is_cancelled());
        assert!(body.source_hint().is_none());
        assert!(detached());
        late_events();
    }
}

#[wasm_bindgen_test]
fn invalid_sizes_are_rejected_and_blob_defaults_remain_file_semantics() {
    for size in [f64::NAN, f64::INFINITY, -1.0, 0.5, 9_007_199_254_740_992.0] {
        let blob = blob(0);
        size_override(&blob, size);
        assert!(BrowserBlobSource::new(blob).is_err());
    }
    let field = BrowserBlobSource::blob_field("file", blob(0)).expect("field");
    assert!(matches!(
        field.value(),
        HttpMultipartValue::File {
            filename: "blob",
            content_type: "application/octet-stream",
            ..
        }
    ));
}

#[wasm_bindgen_test]
async fn canonical_multipart_matches_native_formdata_order_names_mime_and_file_bytes() {
    let parts = Array::new();
    parts.push(&Uint8Array::from(&[1, 2, 3][..]));
    let options = BlobPropertyBag::new();
    options.set_type("Application/OCTET-STREAM");
    let file_blob = Blob::new_with_u8_array_sequence_and_options(&parts, &options).expect("Blob");
    let file = typed_file(&file_blob, "π\n\"\\.bin");
    assert_eq!(file.type_(), "application/octet-stream");
    let expected = JsFuture::from(native_fields(&file))
        .await
        .expect("native fields");
    let file_field = BrowserBlobSource::file_field("same\n\"", file).expect("file field");
    if let HttpMultipartValue::File { source, .. } = file_field.value() {
        let source = source
            .source_hint()
            .expect("hint")
            .downcast_ref::<BrowserBlobSource>()
            .expect("Blob source");
        assert!(source.blob().is_some());
        assert_eq!(source.exact_length(), Some(3));
    } else {
        panic!("file kind");
    }
    let (mut body, header) = HttpMultipartSource::new(
        "hypercolor-browser-test-boundary-0123456789".into(),
        vec![
            HttpMultipartField::text("same\n\"", "one\rtwo"),
            file_field,
            HttpMultipartField::text("same\n\"", "last"),
        ],
    )
    .expect("multipart")
    .into_body(HttpCancellation::new());
    let mut bytes = Vec::new();
    while let Some(chunk) = body.read_chunk(maximum(7)).await.expect("chunk") {
        bytes.extend(chunk);
    }
    let actual = JsFuture::from(canonical_fields(
        &Uint8Array::from(bytes.as_slice()),
        &header.value,
    ))
    .await
    .expect("canonical fields");
    assert_eq!(actual.as_string(), expected.as_string());
}

#[wasm_bindgen_test]
async fn empty_blob_has_no_reader_and_no_hint_after_eof() {
    let _probe = Probe::new("normal");
    let mut body = body(blob(0));
    assert_eq!(body.exact_length(), Some(0));
    assert_eq!(body.read_chunk(maximum(8)).await.expect("EOF"), None);
    assert!(body.source_hint().is_none());
    assert_eq!(read_sizes().length(), 0);
}
