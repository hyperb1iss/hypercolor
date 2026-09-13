#![cfg(target_arch = "wasm32")]

use hypercolor_ui::api::{
    browser_response::BrowserResponseSource,
    http_transport::{HttpBody, HttpBodySource, HttpCancellation},
};
use std::{
    future::Future,
    num::NonZeroUsize,
    task::{Context, Waker},
};
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;
use web_sys::{AbortController, Response};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(inline_js = r#"
export function fixture(mode) {
 let pulls=0,cancels=0;
 const stream=new ReadableStream({pull(controller){
   pulls++;
   if(mode==='pending') return new Promise(()=>{});
   if(mode==='failure') {controller.error(new Error('fixture failure'));return;}
   if(mode==='invalid') {controller.enqueue('not bytes');return;}
   if(pulls===1) controller.enqueue(new Uint8Array(100*1024).fill(91));
   else controller.close();
 },cancel(){cancels++;}}, {highWaterMark:0});
 const response=new Response(stream, {headers:{'Content-Length':'7','Content-Encoding':'gzip'}});
 response.fixturePulls=()=>pulls; response.fixtureCancels=()=>cancels;
 return response;
}
export function pulls(response){return response.fixturePulls();}
export function cancels(response){return response.fixtureCancels();}
export function locked(response){return response.body.locked;}
"#)]
extern "C" {
    fn fixture(mode: &str) -> Response;
    fn pulls(response: &Response) -> u32;
    fn cancels(response: &Response) -> u32;
    fn locked(response: &Response) -> bool;
}

fn body(response: &Response, controller: AbortController) -> HttpBody {
    let source = BrowserResponseSource::new(response, controller).expect("unlocked response");
    assert_eq!(
        source.exact_length(),
        None,
        "decoded response is not Content-Length bytes"
    );
    HttpBody::new(Box::new(source), HttpCancellation::new())
}

#[wasm_bindgen_test]
async fn browser_chunks_are_pulled_on_demand_and_copied_within_requested_capacity() {
    let response = fixture("normal");
    let controller = AbortController::new().expect("controller");
    let signal = controller.signal();
    let mut body = body(&response, controller);
    assert_eq!(pulls(&response), 0);
    assert!(locked(&response));
    let maximum = NonZeroUsize::new(1024).expect("capacity");
    for _ in 0..100 {
        let bytes = body
            .read_chunk(maximum)
            .await
            .expect("chunk")
            .expect("body bytes");
        assert_eq!(bytes, vec![91; 1024]);
        assert_eq!(
            pulls(&response),
            1,
            "no second browser read while buffered bytes remain"
        );
    }
    assert!(body.read_chunk(maximum).await.expect("EOF").is_none());
    assert_eq!(pulls(&response), 2);
    assert!(!locked(&response));
    drop(body);
    assert!(!signal.aborted(), "completed response needs no abort");
}

#[wasm_bindgen_test]
async fn pending_read_drop_aborts_and_unlocks_the_browser_body() {
    let response = fixture("pending");
    let controller = AbortController::new().expect("controller");
    let signal = controller.signal();
    let mut body = body(&response, controller);
    let mut read = Box::pin(body.read_chunk(NonZeroUsize::new(1024).expect("capacity")));
    assert!(
        read.as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    drop(read);
    drop(body);
    assert!(signal.aborted());
    assert!(!locked(&response));
    assert_eq!(cancels(&response), 1);
}

#[wasm_bindgen_test]
async fn browser_failure_and_nonbyte_chunks_abort_without_leaking_a_reader() {
    for mode in ["failure", "invalid"] {
        let response = fixture(mode);
        let controller = AbortController::new().expect("controller");
        let signal = controller.signal();
        let mut body = body(&response, controller);
        assert!(
            body.read_chunk(NonZeroUsize::new(1024).expect("capacity"))
                .await
                .is_err()
        );
        assert!(signal.aborted());
        assert!(!locked(&response));
    }
}

#[wasm_bindgen_test]
fn acquiring_an_already_locked_response_refuses_and_aborts_the_new_exchange() {
    let response = fixture("normal");
    let first = BrowserResponseSource::new(&response, AbortController::new().expect("controller"))
        .expect("first owner");
    let second = AbortController::new().expect("controller");
    let signal = second.signal();
    assert!(BrowserResponseSource::new(&response, second).is_err());
    assert!(signal.aborted());
    assert!(
        locked(&response),
        "failed acquisition cannot steal the first owner"
    );
    drop(first);
    assert!(!locked(&response));
}

#[wasm_bindgen_test]
async fn absent_response_body_finishes_without_aborting_a_completed_exchange() {
    let response = Response::new().expect("empty response");
    let controller = AbortController::new().expect("controller");
    let signal = controller.signal();
    let mut body = body(&response, controller);
    assert!(
        body.read_chunk(NonZeroUsize::new(1024).expect("capacity"))
            .await
            .expect("empty body")
            .is_none()
    );
    drop(body);
    assert!(!signal.aborted());
}
